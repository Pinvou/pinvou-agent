use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

const STORE_VERSION: u32 = 5;
const CONFIG_DEFAULTS_VERSION: u32 = 1;
/// 原生代码会话 sidecar 的 schema 版本；未来字段演进时用于迁移。
const CODE_SESSION_SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum AgentBackend {
    #[default]
    Deepseek,
    CodexAcp,
    ClaudeAcp,
    KimiAcp,
}

// parse/agent_id 是 backend 字符串表示(序列化契约)的公开解析入口,生产链路
// 在用;as_str 仅测试断言别名契约,已单独 #[cfg(test)] 门控。
impl AgentBackend {
    /// ACP Agent 目录的单一注册表。新增后端时只需在这里登记，列表、状态与 UI
    /// 都从同一顺序派生，避免某一端遗漏后只显示部分 Agent。
    pub const ACP_BACKENDS: [Self; 3] = [Self::CodexAcp, Self::ClaudeAcp, Self::KimiAcp];

    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("deepseek") {
            "deepseek" | "pinvou" => Ok(Self::Deepseek),
            "codex-acp" | "codex" => Ok(Self::CodexAcp),
            "claude-acp" | "claude" => Ok(Self::ClaudeAcp),
            "kimi-acp" | "kimi" => Ok(Self::KimiAcp),
            other => anyhow::bail!("不支持的 Agent 后端: {other}"),
        }
    }

    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::CodexAcp => "codex-acp",
            Self::ClaudeAcp => "claude-acp",
            Self::KimiAcp => "kimi-acp",
        }
    }

    pub fn agent_id(self) -> Option<&'static str> {
        match self {
            Self::Deepseek => None,
            Self::CodexAcp => Some("codex"),
            Self::ClaudeAcp => Some("claude"),
            Self::KimiAcp => Some("kimi"),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Deepseek => "品悟",
            Self::CodexAcp => "Codex",
            Self::ClaudeAcp => "Claude Code",
            Self::KimiAcp => "Kimi",
        }
    }

    pub fn is_acp(self) -> bool {
        !matches!(self, Self::Deepseek)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexWorkspaceKind {
    #[default]
    Temporary,
    Project,
}

/// 产品模式类型已上移到 core（store/bridge 策略等多 feature 共用）；
/// 这里 re-export 保持既有 `store::SessionMode` 路径可用。持久化关注点
/// （`code_session` 键的布尔兼容 serde）留在本模块（见 `session_mode_serde`）。
pub use crate::core::session_mode::SessionMode;

/// `code_session` 键的兼容 serde：写布尔（旧格式），读布尔或字符串。
mod session_mode_serde {
    use super::SessionMode;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(mode: &SessionMode, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(mode.is_code())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<SessionMode, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Compat {
            Bool(bool),
            Mode(SessionMode),
        }
        Ok(match Compat::deserialize(deserializer)? {
            Compat::Bool(true) => SessionMode::Code,
            Compat::Bool(false) => SessionMode::Plain,
            Compat::Mode(mode) => mode,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionAgentRecord {
    #[serde(default)]
    pub backend: AgentBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_model_id: Option<String>,
    /// 用户明确选择的 ACP 权限模式。ACP Agent 在 new/load 时可能恢复默认
    /// `agent`，所以 Pinvou 必须在运行时就绪前重新应用该期望值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_mode_id: Option<String>,
    /// 当前 Pinvou 会话最后一次确认成功的 ACP 配置。
    ///
    /// `model` / `mode` 仍保留独立字段用于兼容旧版本；其余 Agent 动态上报的
    /// 配置项统一保存在这里，恢复同一会话时不会被 Agent 默认值覆盖。
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub acp_config_values: HashMap<String, String>,
    /// ACP Agent 的执行目录类型。旧记录没有该字段时按临时会话兼容。
    #[serde(default)]
    pub workspace_kind: CodexWorkspaceKind,
    /// 项目会话保存创建时选定的绝对目录；临时会话目录由 session id 推导。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<PathBuf>,
    /// 创建时锁定的钥匙串快照(§6):全量可访问根(含主根)。旧记录空 =
    /// 单根语义(仅 workspace_path 目录)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_roots: Vec<PathBuf>,
    /// 产品模式（plain/code）。旧记录没有该字段时按 plain 兼容；
    /// 序列化保持原布尔格式（true=code），旧版本应用读新文件不误判。
    #[serde(
        default,
        rename = "code_session",
        with = "session_mode_serde",
        skip_serializing_if = "SessionMode::is_plain"
    )]
    pub mode: SessionMode,
}

/// 原生代码会话的权威 sidecar（per-session 持久化真相源）。
///
/// `session-agents.json` 是运行期辅助索引，损坏/丢失后可以被丢弃重建；而原生
/// 代码会话的类型与项目目录绑定必须跨进程持久存在，否则会话会静默掉回普通
/// 会话、执行根退回私有目录。本 sidecar 存于 session 私有目录
/// `~/.pinvou3/sessions/<sid>/code-session.json`，随会话存续，是辅助索引缺失时
/// 恢复 `SessionAgentRecord` 的依据（见 `restore_missing_code_session_record`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeSessionSidecar {
    #[serde(default = "code_session_sidecar_version")]
    pub version: u32,
    /// 原生代码会话的工作区类型。
    pub workspace_kind: CodexWorkspaceKind,
    /// 项目会话保存的绝对目录；临时会话为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<PathBuf>,
    /// 创建时锁定的钥匙串快照(§6);旧 sidecar 空 = 单根语义。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_roots: Vec<PathBuf>,
    /// 首次绑定时间（Unix 秒）。仅作元信息，不参与恢复语义。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_at: Option<i64>,
}

fn code_session_sidecar_version() -> u32 {
    CODE_SESSION_SIDECAR_VERSION
}

/// 原生代码会话 sidecar 的根目录：`<session-agents.json 父目录>/sessions`。
/// 生产为 `~/.pinvou3/sessions`，测试随 store.path 一并隔离。
/// 启动扫描（mod.rs）与 sidecar 读写共用本函数，保证「扫描根 == 读取根」单一来源。
pub(super) fn code_session_sidecar_root(store_path: &Path) -> PathBuf {
    store_path
        .parent()
        .map(|parent| parent.join("sessions"))
        .unwrap_or_else(crate::platform::paths::sessions_root)
}

/// 该 session 的原生代码会话 sidecar 路径。
fn code_session_sidecar_path(store_path: &Path, session_id: &str) -> PathBuf {
    code_session_sidecar_root(store_path)
        .join(session_id)
        .join("code-session.json")
}

/// 原子写入原生代码会话 sidecar；写入失败逐条记日志并返回 false，不阻断会话
/// 绑定主流程（辅助索引仍然可用，丢失恢复兜底时才依赖 sidecar；缺失的 sidecar
/// 由启动时的 `backfill_missing_code_session_sidecars` 自愈补写）。
fn write_code_session_sidecar(
    store_path: &Path,
    session_id: &str,
    kind: CodexWorkspaceKind,
    workspace_path: Option<PathBuf>,
    workspace_roots: Vec<PathBuf>,
) -> bool {
    let path = code_session_sidecar_path(store_path, session_id);
    let sidecar = CodeSessionSidecar {
        version: CODE_SESSION_SIDECAR_VERSION,
        workspace_kind: kind,
        workspace_path,
        workspace_roots,
        bound_at: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or_default(),
        ),
    };
    match persist_code_session_sidecar(&path, &sidecar) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "[pinvou3-app] 写入原生代码会话 sidecar 失败（{}）: {error:#}",
                path.display()
            );
            false
        }
    }
}

/// 读取原生代码会话 sidecar；不存在、解析失败或 schema 版本高于当前支持版本时
/// 返回 None（按缺失处理，走恢复/回填路径），异常均记日志。
pub(super) fn read_code_session_sidecar(
    store_path: &Path,
    session_id: &str,
) -> Option<CodeSessionSidecar> {
    let path = code_session_sidecar_path(store_path, session_id);
    let payload = fs::read(&path).ok()?;
    match serde_json::from_slice::<CodeSessionSidecar>(&payload) {
        Ok(sidecar) => {
            // 未来高版本格式不能静默按 v1 解析：拒读并按缺失处理，交由恢复/回填
            // 路径用当前版本重写。
            if sidecar.version > CODE_SESSION_SIDECAR_VERSION {
                eprintln!(
                    "[pinvou3-app] 原生代码会话 sidecar 版本 {} 高于当前支持的 {}，按缺失处理（{}）",
                    sidecar.version,
                    CODE_SESSION_SIDECAR_VERSION,
                    path.display()
                );
                return None;
            }
            Some(sidecar)
        }
        Err(error) => {
            eprintln!(
                "[pinvou3-app] 解析原生代码会话 sidecar 失败（{}）: {error:#}",
                path.display()
            );
            None
        }
    }
}

/// 删除原生代码会话 sidecar（会话删除时调用）。
pub(super) fn remove_code_session_sidecar(store_path: &Path, session_id: &str) {
    let sidecar = code_session_sidecar_path(store_path, session_id);
    match fs::remove_file(&sidecar) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "[pinvou3-app] 清理原生代码会话 sidecar 失败（{}）: {error:#}",
            sidecar.display()
        ),
    }
}

fn persist_code_session_sidecar(path: &Path, sidecar: &CodeSessionSidecar) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建会话目录失败: {}", parent.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(sidecar)?)
        .with_context(|| format!("写入 {} 失败", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("保存 {} 失败", path.display()))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentStoreFile {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    sessions: HashMap<String, SessionAgentRecord>,
}

fn store_version() -> u32 {
    STORE_VERSION
}

#[derive(Clone)]
pub struct SessionAgentStore {
    path: PathBuf,
    records: Arc<RwLock<HashMap<String, SessionAgentRecord>>>,
}

impl SessionAgentStore {
    pub fn load() -> Result<Self> {
        let path = crate::platform::paths::pinvou3_home().join("session-agents.json");
        let records = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            serde_json::from_str::<AgentStoreFile>(&raw)
                .with_context(|| format!("解析 {} 失败", path.display()))?
                .sessions
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            records: Arc::new(RwLock::new(records)),
        })
    }

    /// 外部 Agent ACP 是可选能力，它的辅助索引损坏时不能阻断 Pinvou 主程序启动。
    ///
    /// 加载失败时不主动覆盖原始文件；但随后任何一次 `persist`（包括启动时
    /// `AcpPool::new` 恢复缺失的 ACP 记录、或用户创建/更新会话）都会用新内容
    /// 替换它，损坏的内容不会长期保留。
    pub fn load_or_empty() -> Self {
        match Self::load() {
            Ok(store) => store,
            Err(error) => {
                let path = crate::platform::paths::pinvou3_home().join("session-agents.json");
                eprintln!("[pinvou3-app] ACP session index unavailable, starting empty: {error:#}");
                Self {
                    path,
                    records: Arc::new(RwLock::new(HashMap::new())),
                }
            }
        }
    }

    pub fn backend(&self, session_id: &str) -> AgentBackend {
        self.records
            .read()
            .get(session_id)
            .map(|record| record.backend)
            .unwrap_or_default()
    }

    /// 辅助索引文件路径（`session-agents.json`）。sidecar 目录由其派生。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 测试专用：以指定索引路径构造空 store（sidecar 根随之派生，与生产同源）。
    #[cfg(test)]
    pub(crate) fn for_test(path: PathBuf) -> Self {
        Self {
            path,
            records: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get(&self, session_id: &str) -> SessionAgentRecord {
        self.records
            .read()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns session ids bound to the given project workspace for
    /// code-capable sessions: ACP sessions in any mode (they execute file
    /// edits in the project directory even in plain mode) and native sessions
    /// in code mode. Both paths are canonicalized before comparison,
    /// tolerating symlink/trailing-slash differences.
    pub fn code_sessions_in_workspace(&self, root: &Path) -> Vec<String> {
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        self.records
            .read()
            .iter()
            .filter(|(_, record)| {
                if record.workspace_kind != CodexWorkspaceKind::Project {
                    return false;
                }
                if !record.mode.is_code() && !record.backend.is_acp() {
                    return false;
                }
                record
                    .workspace_path
                    .as_deref()
                    .map(|path| {
                        let path =
                            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                        path == canonical
                    })
                    .unwrap_or(false)
            })
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }

    /// 在 ACP 会话创建时永久绑定 Agent 与执行目录。
    ///
    /// `set_acp_workspace` 是当前前端创建会话时的主入口；这个更窄的 API 保留给
    /// 后续仅切换后端、不变更工作区的场景。
    #[allow(dead_code)]
    pub fn set_backend(&self, session_id: &str, backend: AgentBackend) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            if record.backend != backend {
                record.backend = backend;
                record.acp_session_id = None;
                record.acp_model_id = None;
                record.acp_mode_id = None;
                record.acp_config_values.clear();
                record.workspace_kind = CodexWorkspaceKind::Temporary;
                record.workspace_path = None;
            }
        }
        self.persist()
    }

    /// ACP session 一旦建立就不允许换 Agent 或目录，避免同一个 Agent 上下文跨
    /// 后端或跨项目漂移。
    pub fn set_acp_workspace(
        &self,
        session_id: &str,
        backend: AgentBackend,
        kind: CodexWorkspaceKind,
        workspace_path: Option<PathBuf>,
        workspace_roots: Vec<PathBuf>,
    ) -> Result<()> {
        if !backend.is_acp() {
            anyhow::bail!("ACP 会话不能绑定非 ACP 后端");
        }
        if kind == CodexWorkspaceKind::Project && workspace_path.is_none() {
            anyhow::bail!("项目会话缺少 ACP 工作目录");
        }
        if kind == CodexWorkspaceKind::Temporary && workspace_path.is_some() {
            anyhow::bail!("临时会话不能保存项目工作目录");
        }
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            if record.acp_session_id.is_some()
                && (record.backend != backend
                    || record.workspace_kind != kind
                    || record.workspace_path != workspace_path)
            {
                anyhow::bail!("ACP 会话已开始，不能更换 Agent 或工作目录；请新建会话");
            }
            record.backend = backend;
            record.workspace_kind = kind;
            record.workspace_path = workspace_path;
            // 钥匙串快照(§6):项目会话存全量根;临时会话恒空。
            record.workspace_roots = if kind == CodexWorkspaceKind::Project {
                workspace_roots
            } else {
                Vec::new()
            };
            // ACP 会话不是代码模式会话：绑定 ACP 时重置为 plain 模式，
            // 避免 is_code_session() 误判、且 restore 时不会拒绝 ACP 覆盖。
            record.mode = SessionMode::Plain;
        }
        // 先持久化辅助索引，再清理权威 sidecar：persist 失败时 sidecar 仍在，与
        // 磁盘索引保持一致，不会出现「sidecar 已删、索引未更新」的中间态；若 sidecar
        // 清理失败，残留 sidecar 会在下次启动扫描时被识别为 ACP 会话残留并清理
        // （见 mod.rs `restore_code_native_sessions_from_sidecars`）。
        self.persist()?;
        // 该会话不再是原生代码会话：清理权威 sidecar，防止辅助索引重建时误恢复。
        remove_code_session_sidecar(&self.path, session_id);
        Ok(())
    }

    /// 绑定“代码”模块的原生（品悟 Engine）会话。临时会话目录由 session id 推导；
    /// 项目会话保存创建时选定的绝对目录（调用前须经 `validate_codex_project_workspace`
    /// 校验）。与 ACP 会话同样遵循“会话开始后不可换 Agent 或工作目录”。
    pub fn bind_code_native_session(
        &self,
        session_id: &str,
        kind: CodexWorkspaceKind,
        workspace_path: Option<PathBuf>,
        workspace_roots: Vec<PathBuf>,
    ) -> Result<()> {
        if kind == CodexWorkspaceKind::Project && workspace_path.is_none() {
            anyhow::bail!("项目会话缺少工作目录");
        }
        if kind == CodexWorkspaceKind::Temporary && workspace_path.is_some() {
            anyhow::bail!("临时会话不能保存项目工作目录");
        }
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            if record.acp_session_id.is_some() && record.backend.is_acp() {
                anyhow::bail!("ACP 会话已开始，不能更换 Agent；请新建会话");
            }
            // 已绑定的原生代码会话不允许改绑到其他工作区（同值重复绑定幂等放行）。
            if record.mode.is_code()
                && (record.workspace_kind != kind || record.workspace_path != workspace_path)
            {
                anyhow::bail!("代码会话已开始，不能更换工作目录；请新建会话");
            }
            record.backend = AgentBackend::Deepseek;
            record.workspace_kind = kind;
            record.workspace_path = workspace_path.clone();
            // 钥匙串快照(§6):项目会话存全量根;临时会话恒空。
            record.workspace_roots = if kind == CodexWorkspaceKind::Project {
                workspace_roots
            } else {
                Vec::new()
            };
            record.mode = SessionMode::Code;
        }
        self.persist()?;
        // 权威 sidecar：辅助索引损坏/丢失后据此恢复原生代码会话类型与项目绑定。
        // 写失败已逐条记日志；缺失的 sidecar 由启动时回填自愈补写。
        let record_roots = self
            .records
            .read()
            .get(session_id)
            .map(|record| record.workspace_roots.clone())
            .unwrap_or_default();
        write_code_session_sidecar(&self.path, session_id, kind, workspace_path, record_roots);
        Ok(())
    }

    /// "对齐到项目"(§9.7)的钥匙串替换:整体改写创建快照(权限只增不减的
    /// 约束由调用方/命令层按语义保证)。原生代码会话同步重写权威 sidecar
    /// (保留 bound_at);ACP 会话无 sidecar,只写索引。记录不存在或未绑定
    /// 工作区(临时会话)返回 Ok(false),调用方按普通绑定通道处理。
    pub fn set_session_workspace_roots(
        &self,
        session_id: &str,
        workspace_roots: Vec<PathBuf>,
    ) -> Result<bool> {
        {
            let mut records = self.records.write();
            let Some(record) = records.get_mut(session_id) else {
                return Ok(false);
            };
            if record.workspace_kind != CodexWorkspaceKind::Project
                || record.workspace_path.is_none()
            {
                return Ok(false);
            }
            record.workspace_roots = workspace_roots.clone();
        }
        self.persist()?;
        let record = self.get(session_id);
        if record.mode.is_code() {
            write_code_session_sidecar(
                &self.path,
                session_id,
                record.workspace_kind,
                record.workspace_path,
                workspace_roots,
            );
        }
        Ok(true)
    }

    /// 会话创建时锁定的钥匙串快照(§6):全量可访问根(含主根);旧记录/
    /// 临时会话/无记录 = 空(单根语义,调用方按 cwd 居首归一)。
    pub fn session_workspace_roots(&self, session_id: &str) -> Vec<PathBuf> {
        self.records
            .read()
            .get(session_id)
            .map(|record| record.workspace_roots.clone())
            .unwrap_or_default()
    }

    /// 该会话的产品模式（plain/code）；无记录时按 plain 缺省
    /// （与历史 `is_code_session` 缺省 false 等价）。
    pub fn session_mode(&self, session_id: &str) -> SessionMode {
        self.records
            .read()
            .get(session_id)
            .map(|record| record.mode)
            .unwrap_or_default()
    }

    /// 是否为“代码”模块的原生（品悟 Engine）会话。
    pub fn is_code_session(&self, session_id: &str) -> bool {
        self.session_mode(session_id).is_code()
    }

    /// 原生代码会话绑定的项目目录；非代码会话或临时会话返回 None。
    /// 这是“两个根”的唯一判定入口：执行根（engine/shell）命中它时解析到项目目录，
    /// 账本根（附件/审计/产物）命中它时必须改用会话私有目录。
    pub fn code_project_workspace(&self, session_id: &str) -> Option<PathBuf> {
        let record = self.get(session_id);
        if record.mode.is_code() && record.workspace_kind == CodexWorkspaceKind::Project {
            record.workspace_path
        } else {
            None
        }
    }

    /// 折叠键前缀匹配 + 原样后缀:匹配经共享 `filesystem_path_identity_key`
    /// 折叠(Windows 折叠分隔符与大小写,仅大小写改名不再漏配),后缀按组件数
    /// 从原路径切回,保留子目录原有大小写。返回 `None` = 不在 `from` 之下;
    /// 空后缀 = 路径本身就是 `from`。
    fn rebind_relative_suffix(path: &Path, from: &Path) -> Option<PathBuf> {
        let path_key = crate::platform::os::filesystem_path_identity_key(&path.to_string_lossy());
        let from_key = crate::platform::os::filesystem_path_identity_key(&from.to_string_lossy());
        let path_trim = path_key.trim_end_matches('/');
        let from_trim = from_key.trim_end_matches('/');
        let covered = from_trim.is_empty()
            || path_trim == from_trim
            || path_trim.starts_with(&format!("{from_trim}/"));
        if !covered {
            return None;
        }
        Some(path.components().skip(from.components().count()).collect())
    }

    /// 列出绑定在 `from` 前缀下的项目会话（目录重绑定的候选集；`from` 通常
    /// 已在磁盘上消失，因此按折叠键前缀匹配而非 canonicalize 比较）。
    /// 除辅助索引外同时扫描 code-session sidecar 目录：索引损坏/丢失时全部
    /// 原生代码会话都是索引外孤儿，只扫索引会让重绑定栅栏与元数据重放集体
    /// 漏保（评审 #463 M6）。同一 session_id 索引记录优先，孤儿仅补差集。
    pub fn sessions_under_workspace(&self, from: &Path) -> Vec<(String, PathBuf)> {
        let mut matched: Vec<(String, PathBuf)> = self
            .records
            .read()
            .iter()
            .filter_map(|(session_id, record)| {
                if record.workspace_kind != CodexWorkspaceKind::Project {
                    return None;
                }
                let path = record.workspace_path.as_ref()?;
                Self::rebind_relative_suffix(path, from).map(|_| (session_id.clone(), path.clone()))
            })
            .collect();
        // 索引外孤儿 sidecar:启动恢复以 sidecar 为权威,候选集必须同口径
        // (扫法与 rebind_workspace_prefix 的 sidecar 重写段一致)。
        let sidecar_root = code_session_sidecar_root(&self.path);
        if let Ok(entries) = fs::read_dir(&sidecar_root) {
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let Some(session_id) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if matched.iter().any(|(sid, _)| *sid == session_id) {
                    continue;
                }
                let Some(sidecar) = read_code_session_sidecar(&self.path, &session_id) else {
                    continue;
                };
                if sidecar.workspace_kind != CodexWorkspaceKind::Project {
                    continue;
                }
                let Some(path) = sidecar.workspace_path else {
                    continue;
                };
                if Self::rebind_relative_suffix(&path, from).is_some() {
                    matched.push((session_id, path));
                }
            }
        }
        matched
    }

    /// 由候选绑定路径算重绑定目标:`from` 前缀下的路径平移后缀到 `to`;已在
    /// `to` 前缀下的路径(上一轮已平移、元数据未同步的重试候选)原样返回。
    /// 两者都不命中返回 None(不可能是候选)。
    pub fn rebind_target_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
        if let Some(suffix) = Self::rebind_relative_suffix(path, from) {
            return Some(if suffix.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(suffix)
            });
        }
        Self::rebind_relative_suffix(path, to).map(|_| path.to_path_buf())
    }

    /// 目录重绑定（修断链通道）：把绑定在 `from` 前缀下的项目会话整体平移到
    /// `to`。与 `set_acp_workspace`/`bind_code_native_session` 的“会话开始后
    /// 不可换目录”不同，本方法绕过该约束——仅当目录已失效（或用户在旧目录
    /// 仍存在时显式确认）时由命令层调用，命令层负责活跃回合栅栏与目标校验。
    ///
    /// 三处一致中的前两处在此落盘：辅助索引（session-agents.json）与权威
    /// sidecar（code-session.json，含索引外孤儿——启动扫描按 sidecar 恢复，
    /// 漏写会让旧值复活）；第三处 SavedSession 元数据由命令层经 SessionStore
    /// 写入。幂等：from→to 重跑无匹配即空操作。返回受影响 (session_id, 新路径)。
    pub fn rebind_workspace_prefix(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<Vec<(String, PathBuf)>> {
        if from == to {
            return Ok(Vec::new());
        }
        let mut affected: Vec<(String, PathBuf)> = Vec::new();
        {
            let mut records = self.records.write();
            for (session_id, record) in records.iter_mut() {
                if record.workspace_kind != CodexWorkspaceKind::Project {
                    continue;
                }
                let Some(path) = record.workspace_path.clone() else {
                    continue;
                };
                let Some(suffix) = Self::rebind_relative_suffix(&path, from) else {
                    continue;
                };
                let next = if suffix.as_os_str().is_empty() {
                    to.to_path_buf()
                } else {
                    to.join(suffix)
                };
                record.workspace_path = Some(next.clone());
                // 钥匙串快照同步平移:from 前缀下的根换到 to,其余原样(§6
                // 快照随绑定迁移;from 外的根不受影响)。
                for root in record.workspace_roots.iter_mut() {
                    if let Some(root_suffix) = Self::rebind_relative_suffix(root, from) {
                        *root = if root_suffix.as_os_str().is_empty() {
                            to.to_path_buf()
                        } else {
                            to.join(root_suffix)
                        };
                    }
                }
                affected.push((session_id.clone(), next));
            }
        }
        self.persist()?;
        // sidecar 重写：原生代码会话的权威绑定。ACP 会话本就无 sidecar（绑定
        // 时即清除）；索引外孤儿 sidecar 也要改——否则重启恢复会用旧目录复活。
        // 写失败记日志且不标记已重写（下面的补写段会重试），不静默按成功计。
        let mut sidecar_rewritten: Vec<String> = Vec::new();
        let sidecar_root = code_session_sidecar_root(&self.path);
        if let Ok(entries) = fs::read_dir(&sidecar_root) {
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let Some(session_id) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let Some(sidecar) = read_code_session_sidecar(&self.path, &session_id) else {
                    continue;
                };
                if sidecar.workspace_kind != CodexWorkspaceKind::Project {
                    continue;
                }
                let Some(path) = sidecar.workspace_path.as_ref() else {
                    continue;
                };
                let Some(suffix) = Self::rebind_relative_suffix(path, from) else {
                    continue;
                };
                let next = if suffix.as_os_str().is_empty() {
                    to.to_path_buf()
                } else {
                    to.join(suffix)
                };
                let rebound_roots: Vec<PathBuf> = sidecar
                    .workspace_roots
                    .iter()
                    .map(|root| {
                        match Self::rebind_relative_suffix(root, from) {
                            Some(root_suffix) if root_suffix.as_os_str().is_empty() => {
                                to.to_path_buf()
                            }
                            Some(root_suffix) => to.join(root_suffix),
                            None => root.clone(),
                        }
                    })
                    .collect();
                if let Err(error) = persist_code_session_sidecar(
                    &code_session_sidecar_path(&self.path, &session_id),
                    &CodeSessionSidecar {
                        version: CODE_SESSION_SIDECAR_VERSION,
                        workspace_kind: CodexWorkspaceKind::Project,
                        workspace_path: Some(next.clone()),
                        workspace_roots: rebound_roots,
                        bound_at: sidecar.bound_at,
                    },
                ) {
                    // 旧 sidecar 仍在盘上,重启恢复会复活旧目录;记日志并让
                    // 索引内会话走下面的补写段重试(评审 #463 minor)。
                    eprintln!(
                        "[pinvou3-app] 重绑定改写原生代码会话 sidecar 失败（{session_id}）: {error:#}"
                    );
                } else {
                    sidecar_rewritten.push(session_id.clone());
                }
                // 索引缺失的孤儿会话也计入受影响名单（索引里改不到它们）。
                if !affected.iter().any(|(sid, _)| *sid == session_id) {
                    affected.push((session_id, next));
                }
            }
        }
        // 索引内已改绑的原生代码会话补写 sidecar（失败仅记日志，启动回填自愈）。
        // 跳过上面已重写的会话:重写段保留了原 bound_at,这里再以 now 回填
        // 是双写 + 丢失首次绑定时间(评审 #463 minor)。
        {
            let records = self.records.read();
            for (session_id, path) in &affected {
                if sidecar_rewritten.iter().any(|sid| sid == session_id) {
                    continue;
                }
                if records
                    .get(session_id)
                    .is_some_and(|record| record.mode.is_code())
                {
                    let record_roots = records
                        .get(session_id)
                        .map(|record| record.workspace_roots.clone())
                        .unwrap_or_default();
                    write_code_session_sidecar(
                        &self.path,
                        session_id,
                        CodexWorkspaceKind::Project,
                        Some(path.clone()),
                        record_roots,
                    );
                }
            }
        }
        Ok(affected)
    }

    pub fn set_acp_session(
        &self,
        session_id: &str,
        acp_session_id: String,
        model_id: Option<String>,
        config_values: HashMap<String, String>,
    ) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record.acp_session_id = Some(acp_session_id);
            record.acp_model_id = model_id
                .clone()
                .or_else(|| config_values.get("model").cloned());
            record.acp_mode_id = config_values.get("mode").cloned();
            record.acp_config_values = config_values;
            if let Some(model_id) = model_id {
                record
                    .acp_config_values
                    .insert("model".to_string(), model_id);
            }
        }
        self.persist()
    }

    pub fn set_acp_model(&self, session_id: &str, model_id: Option<String>) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record.acp_model_id = model_id.clone();
            match model_id {
                Some(model_id) => {
                    record
                        .acp_config_values
                        .insert("model".to_string(), model_id);
                }
                None => {
                    record.acp_config_values.remove("model");
                }
            }
        }
        self.persist()
    }

    pub fn set_acp_mode(&self, session_id: &str, mode_id: Option<String>) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record.acp_mode_id = mode_id.clone();
            match mode_id {
                Some(mode_id) => {
                    record.acp_config_values.insert("mode".to_string(), mode_id);
                }
                None => {
                    record.acp_config_values.remove("mode");
                }
            }
        }
        self.persist()
    }

    pub fn set_acp_config_value(
        &self,
        session_id: &str,
        config_id: &str,
        value_id: &str,
    ) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record
                .acp_config_values
                .insert(config_id.to_string(), value_id.to_string());
            if config_id == "model" {
                record.acp_model_id = Some(value_id.to_string());
            } else if config_id == "mode" {
                record.acp_mode_id = Some(value_id.to_string());
            }
        }
        self.persist()
    }

    pub fn clear_acp_config_value(&self, session_id: &str, config_id: &str) -> Result<()> {
        {
            let mut records = self.records.write();
            let Some(record) = records.get_mut(session_id) else {
                return Ok(());
            };
            record.acp_config_values.remove(config_id);
            if config_id == "model" {
                record.acp_model_id = None;
            } else if config_id == "mode" {
                record.acp_mode_id = None;
            }
        }
        self.persist()
    }

    pub(super) fn restore_missing_acp_record(
        &self,
        session_id: &str,
        recovered: SessionAgentRecord,
    ) -> Result<()> {
        if !recovered.backend.is_acp()
            || recovered
                .acp_session_id
                .as_deref()
                .is_none_or(str::is_empty)
        {
            anyhow::bail!("恢复的 ACP 会话索引不完整");
        }
        {
            let mut records = self.records.write();
            if records
                .get(session_id)
                .is_some_and(|record| record.backend.is_acp())
            {
                return Ok(());
            }
            records.insert(session_id.to_string(), recovered);
        }
        self.persist()
    }

    pub fn remove(&self, session_id: &str) -> Result<()> {
        self.records.write().remove(session_id);
        self.persist()?;
        // 删除会话时同步清理权威 sidecar，避免残留的 sidecar 让重建后的索引
        // 误恢复一个已删除的原生代码会话。
        remove_code_session_sidecar(&self.path, session_id);
        Ok(())
    }

    /// 回填缺失的原生代码会话 sidecar（启动自愈）。
    ///
    /// 两类来源：sidecar 持久化修复前构建创建的存量会话从未写过 sidecar；绑定时
    /// sidecar 写失败只记日志未补写。索引记录 `code_session=true` 而 sidecar 缺失
    /// 时按索引补写；返回成功补写的数量（写失败已逐条记日志，不计入）。
    pub fn backfill_missing_code_session_sidecars(&self) -> usize {
        let records = self.records.read().clone();
        let mut backfilled = 0usize;
        for (session_id, record) in records {
            if !record.mode.is_code() {
                continue;
            }
            if read_code_session_sidecar(&self.path, &session_id).is_some() {
                continue;
            }
            if write_code_session_sidecar(
                &self.path,
                &session_id,
                record.workspace_kind,
                record.workspace_path,
                record.workspace_roots,
            ) {
                backfilled += 1;
                eprintln!("[pinvou3-app] 回填原生代码会话 sidecar: {session_id}");
            }
        }
        backfilled
    }

    /// 从权威 sidecar 恢复原生代码会话记录（辅助索引缺失/损坏时的兜底）。
    ///
    /// 与 ACP 的 [`Self::restore_missing_acp_record`] 对称：sidecar 是长期权威
    /// 依据，辅助索引只负责加速。恢复成功即持久化回 `session-agents.json`，
    /// 使后续读取不再依赖 sidecar。返回是否真实发生了恢复：索引已持有
    /// code 模式记录时返回 `Ok(false)`，调用方不得把它计入恢复信号。
    pub fn restore_missing_code_session_record(
        &self,
        session_id: &str,
        recovered: CodeSessionSidecar,
    ) -> Result<bool> {
        let record = SessionAgentRecord {
            backend: AgentBackend::Deepseek,
            workspace_kind: recovered.workspace_kind,
            workspace_path: recovered.workspace_path,
            workspace_roots: recovered.workspace_roots,
            mode: SessionMode::Code,
            ..Default::default()
        };
        if record.workspace_kind == CodexWorkspaceKind::Project && record.workspace_path.is_none() {
            anyhow::bail!("恢复的原生代码会话缺少项目工作目录");
        }
        if record.workspace_kind == CodexWorkspaceKind::Temporary && record.workspace_path.is_some()
        {
            anyhow::bail!("恢复的原生代码会话临时目录不应保存项目工作目录");
        }
        {
            let mut records = self.records.write();
            if records
                .get(session_id)
                .is_some_and(|record| record.mode.is_code())
            {
                return Ok(false);
            }
            if records
                .get(session_id)
                .is_some_and(|record| record.backend.is_acp())
            {
                anyhow::bail!("会话已是 ACP 会话，拒绝用原生代码会话 sidecar 覆盖");
            }
            records.insert(session_id.to_string(), record);
        }
        self.persist()?;
        Ok(true)
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let value = AgentStoreFile {
            version: STORE_VERSION,
            sessions: self.records.read().clone(),
        };
        fs::write(&tmp, serde_json::to_vec_pretty(&value)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AcpConfigDefaultsFile {
    #[serde(default = "config_defaults_version")]
    version: u32,
    #[serde(default)]
    agents: HashMap<AgentBackend, HashMap<String, String>>,
}

fn config_defaults_version() -> u32 {
    CONFIG_DEFAULTS_VERSION
}

/// 用户为每个 ACP Agent 选择的新会话默认配置。
///
/// ACP 只定义 session 级配置，不负责跨 session 持久化；Pinvou 作为 client 将
/// 用户成功应用过的配置按 Agent 隔离保存，并在新建 session 后重新应用。
#[derive(Clone)]
pub struct AcpConfigDefaultsStore {
    path: PathBuf,
    records: Arc<RwLock<HashMap<AgentBackend, HashMap<String, String>>>>,
}

impl AcpConfigDefaultsStore {
    pub fn load() -> Result<Self> {
        let path = crate::platform::paths::pinvou3_home().join("acp-agent-defaults.json");
        let records = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            serde_json::from_str::<AcpConfigDefaultsFile>(&raw)
                .with_context(|| format!("解析 {} 失败", path.display()))?
                .agents
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            records: Arc::new(RwLock::new(records)),
        })
    }

    /// 默认值文件损坏不能阻断 Agent 启动；保留原文件，下一次用户成功修改配置时
    /// 会重新生成可用内容。
    pub fn load_or_empty() -> Self {
        match Self::load() {
            Ok(store) => store,
            Err(error) => {
                let path = crate::platform::paths::pinvou3_home().join("acp-agent-defaults.json");
                eprintln!(
                    "[pinvou3-app] ACP agent defaults unavailable, starting empty: {error:#}"
                );
                Self {
                    path,
                    records: Arc::new(RwLock::new(HashMap::new())),
                }
            }
        }
    }

    pub fn get(&self, backend: AgentBackend) -> HashMap<String, String> {
        self.records
            .read()
            .get(&backend)
            .cloned()
            .unwrap_or_default()
    }

    pub fn has_backend(&self, backend: AgentBackend) -> bool {
        self.records.read().contains_key(&backend)
    }

    pub fn set(&self, backend: AgentBackend, config_id: &str, value_id: &str) -> Result<()> {
        if !backend.is_acp() {
            anyhow::bail!("不能为非 ACP Agent 保存配置默认值");
        }
        self.records
            .write()
            .entry(backend)
            .or_default()
            .insert(config_id.to_string(), value_id.to_string());
        self.persist()
    }

    pub fn set_all_if_absent(
        &self,
        backend: AgentBackend,
        values: HashMap<String, String>,
    ) -> Result<bool> {
        if !backend.is_acp() || values.is_empty() {
            return Ok(false);
        }
        {
            let mut records = self.records.write();
            if records.contains_key(&backend) {
                return Ok(false);
            }
            records.insert(backend, values);
        }
        self.persist()?;
        Ok(true)
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let value = AcpConfigDefaultsFile {
            version: CONFIG_DEFAULTS_VERSION,
            agents: self.records.read().clone(),
        };
        fs::write(&tmp, serde_json::to_vec_pretty(&value)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

pub fn validate_codex_project_workspace(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("请选择项目目录");
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Codex 项目目录不存在或不可访问: {}", path.display()))?;
    if !canonical.is_dir() {
        anyhow::bail!("Codex 工作目录必须是文件夹: {}", canonical.display());
    }
    // Windows 的 canonicalize 返回 \\?\ 前缀的 verbatim 路径；ACP agent（如 kimi acp）的
    // 工作目录校验不识别该形式，会把一切相对路径误判为“工作目录之外”而拒绝读取。
    // 统一归一化为常规盘符路径（非 Windows 平台为恒等映射）。
    Ok(crate::platform::os::platform_compat_path(
        &canonical.to_string_lossy(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_aliases_are_stable() {
        assert_eq!(AgentBackend::parse(None).unwrap(), AgentBackend::Deepseek);
        assert_eq!(
            AgentBackend::parse(Some("pinvou")).unwrap(),
            AgentBackend::Deepseek
        );
        assert_eq!(
            AgentBackend::parse(Some("codex")).unwrap(),
            AgentBackend::CodexAcp
        );
        assert_eq!(AgentBackend::CodexAcp.as_str(), "codex-acp");
        assert_eq!(
            AgentBackend::parse(Some("claude")).unwrap(),
            AgentBackend::ClaudeAcp
        );
        assert_eq!(
            AgentBackend::parse(Some("kimi-acp")).unwrap(),
            AgentBackend::KimiAcp
        );
        assert!(AgentBackend::ClaudeAcp.is_acp());
    }

    #[test]
    fn empty_store_defaults_to_deepseek() {
        let store = SessionAgentStore {
            path: PathBuf::from("/tmp/unused-session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert_eq!(store.backend("missing"), AgentBackend::Deepseek);
    }

    #[test]
    fn code_sessions_in_workspace_matches_project_code_records_only() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-agent-store-ws-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let record = |kind, path, mode, backend| SessionAgentRecord {
            backend,
            mode,
            workspace_kind: kind,
            workspace_path: path,
            workspace_roots: Vec::new(),
            ..SessionAgentRecord::default()
        };
        let store = SessionAgentStore {
            path: dir.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::from([
                (
                    "acp-proj".to_string(),
                    record(
                        CodexWorkspaceKind::Project,
                        Some(dir.clone()),
                        SessionMode::Plain,
                        AgentBackend::CodexAcp,
                    ),
                ),
                (
                    "native-proj".to_string(),
                    record(
                        CodexWorkspaceKind::Project,
                        Some(dir.clone()),
                        SessionMode::Code,
                        AgentBackend::Deepseek,
                    ),
                ),
                (
                    "temp-code".to_string(),
                    record(
                        CodexWorkspaceKind::Temporary,
                        None,
                        SessionMode::Code,
                        AgentBackend::Deepseek,
                    ),
                ),
                (
                    "plain-chat".to_string(),
                    record(
                        CodexWorkspaceKind::Project,
                        Some(dir.clone()),
                        SessionMode::Plain,
                        AgentBackend::Deepseek,
                    ),
                ),
            ]))),
        };
        let mut sessions = store.code_sessions_in_workspace(&dir);
        sessions.sort();
        assert_eq!(
            sessions,
            vec!["acp-proj".to_string(), "native-proj".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_record_defaults_to_temporary_workspace() {
        let record: SessionAgentRecord = serde_json::from_value(serde_json::json!({
            "backend": "codex-acp",
            "acp_session_id": "legacy-acp"
        }))
        .unwrap();
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Temporary);
        assert_eq!(record.workspace_path, None);
        assert_eq!(record.acp_mode_id, None);
        assert!(record.acp_config_values.is_empty());
        assert!(!record.mode.is_code());
    }

    #[test]
    fn session_mode_deserializes_legacy_bool_and_serializes_as_bool() {
        // 旧格式布尔键：true=code、false=plain，缺字段按 plain 兼容（缺字段
        // 已由 legacy_record_defaults_to_temporary_workspace 覆盖）。
        let code: SessionAgentRecord = serde_json::from_value(serde_json::json!({
            "backend": "deepseek",
            "code_session": true
        }))
        .unwrap();
        assert_eq!(code.mode, SessionMode::Code);
        let plain: SessionAgentRecord = serde_json::from_value(serde_json::json!({
            "backend": "deepseek",
            "code_session": false
        }))
        .unwrap();
        assert_eq!(plain.mode, SessionMode::Plain);
        // 序列化保持原布尔键与省略语义：code 写 true，plain 不写该键，
        // 旧版本应用读新文件不误判。
        let code_json = serde_json::to_value(&code).unwrap();
        assert_eq!(code_json["code_session"], serde_json::json!(true));
        let plain_json = serde_json::to_value(&plain).unwrap();
        assert!(plain_json.get("code_session").is_none());
        // 前向兼容：未来 kebab-case 字符串格式也能读出。
        let future: SessionAgentRecord = serde_json::from_value(serde_json::json!({
            "backend": "deepseek",
            "code_session": "code"
        }))
        .unwrap();
        assert_eq!(future.mode, SessionMode::Code);
    }

    #[test]
    fn project_workspace_must_exist_and_be_a_directory() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-workspace-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let validated = validate_codex_project_workspace(&root).unwrap();
        assert!(validated.is_dir());
        assert_eq!(
            validated.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
        // 回归断言：交给 ACP agent 的工作目录不得保留 \\?\ verbatim 前缀，
        // 否则 kimi acp 会把相对路径误判为“工作目录之外”。该断言全平台有效——
        // 非 Windows 的 canonicalize 本就不产生该前缀，Windows 上由 platform_compat_path 归一化。
        assert!(
            !validated.to_string_lossy().starts_with(r"\\?\"),
            "validated workspace must not keep the verbatim prefix: {}",
            validated.display()
        );
        assert!(validate_codex_project_workspace(&root.join("missing")).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn started_codex_session_cannot_change_workspace() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-store-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        store
            .set_acp_session("session-1", "acp-1".to_string(), None, HashMap::new())
            .unwrap();
        assert!(
            store
                .set_acp_workspace(
                    "session-1",
                    AgentBackend::CodexAcp,
                    CodexWorkspaceKind::Temporary,
                    None,
                Vec::new(),
            )
                .is_err()
        );
        assert!(
            store
                .set_acp_workspace(
                    "session-1",
                    AgentBackend::ClaudeAcp,
                    CodexWorkspaceKind::Project,
                    Some(root.clone()),
                Vec::new(),
            )
                .is_err()
        );
        assert_eq!(store.get("session-1").backend, AgentBackend::CodexAcp);
        assert_eq!(
            store.get("session-1").workspace_path.as_deref(),
            Some(root.as_path())
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_binding_marks_temporary_deepseek_session() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert!(!store.is_code_session("session-1"));
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();

        let record = store.get("session-1");
        assert_eq!(record.backend, AgentBackend::Deepseek);
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Temporary);
        assert_eq!(record.workspace_path, None);
        assert!(record.mode.is_code());
        assert!(store.is_code_session("session-1"));
        // 原生绑定不需要 Agent 上下文，同值重复绑定保持幂等。
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();
        assert!(store.is_code_session("session-1"));

        let persisted: AgentStoreFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let record = &persisted.sessions["session-1"];
        assert!(record.mode.is_code());
        assert_eq!(record.backend, AgentBackend::Deepseek);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn started_acp_session_cannot_be_rebound_to_code_native() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-rebind-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
                Vec::new(),
            )
            .unwrap();
        store
            .set_acp_session("session-1", "acp-1".to_string(), None, HashMap::new())
            .unwrap();
        assert!(
            store
                .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
                .is_err()
        );
        let record = store.get("session-1");
        assert_eq!(record.backend, AgentBackend::CodexAcp);
        assert!(!record.mode.is_code());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_project_binding_validates_kind_and_path() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-project-bind-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        // kind 与 path 必须配套。
        assert!(
            store
                .bind_code_native_session("session-1", CodexWorkspaceKind::Project, None, Vec::new())
                .is_err()
        );
        assert!(
            store
                .bind_code_native_session(
                    "session-1",
                    CodexWorkspaceKind::Temporary,
                    Some(root.clone()),
                Vec::new(),
            )
                .is_err()
        );
        assert!(!store.is_code_session("session-1"));

        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        let record = store.get("session-1");
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(record.workspace_path.as_deref(), Some(root.as_path()));
        assert!(record.mode.is_code());

        // 已绑定的代码会话不可改绑工作区；同值重复绑定幂等。
        assert!(
            store
                .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
                .is_err()
        );
        assert!(
            store
                .bind_code_native_session(
                    "session-1",
                    CodexWorkspaceKind::Project,
                    Some(root.join("other")),
                Vec::new(),
            )
                .is_err()
        );
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        let record = store.get("session-1");
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(record.workspace_path.as_deref(), Some(root.as_path()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rebind_workspace_prefix_rewrites_index_and_sidecars() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-rebind-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let from = root.join("from");
        let to = root.join("to");
        fs::create_dir_all(from.join("sub")).unwrap();
        fs::create_dir_all(&to).unwrap();

        // s1:原生代码会话,绑定 = from 本身(写 sidecar);
        // s2:ACP 项目会话,绑定 = from/sub(无 sidecar);
        // s3:绑定在别处,不受影响;前缀边界:from-x 不得命中 from。
        store
            .bind_code_native_session("s1", CodexWorkspaceKind::Project, Some(from.clone()), Vec::new())
            .unwrap();
        store
            .set_acp_workspace(
                "s2",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(from.join("sub")),
                Vec::new(),
            )
            .unwrap();
        store
            .bind_code_native_session(
                "s3",
                CodexWorkspaceKind::Project,
                Some(root.join("elsewhere")),
                Vec::new(),
            )
            .unwrap();
        store
            .bind_code_native_session("s4", CodexWorkspaceKind::Project, Some(root.join("from-x")), Vec::new())
            .unwrap();

        let matched = store.sessions_under_workspace(&from);
        // s1 在索引与 sidecar 双处命中,按 session_id 去重只计一次(索引优先)。
        assert_eq!(matched.len(), 2);

        let bound_at_before =
            read_code_session_sidecar(&store.path, "s1").and_then(|sidecar| sidecar.bound_at);
        assert!(bound_at_before.is_some());

        let affected = store.rebind_workspace_prefix(&from, &to).unwrap();
        let mut ids: Vec<&str> = affected.iter().map(|(sid, _)| sid.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["s1", "s2"]);
        assert_eq!(
            store.get("s1").workspace_path.as_deref(),
            Some(to.as_path())
        );
        assert_eq!(
            store.get("s2").workspace_path.as_deref(),
            Some(to.join("sub").as_path())
        );
        assert_eq!(
            store.get("s3").workspace_path.as_deref(),
            Some(root.join("elsewhere").as_path()),
            "prefix 外会话不动"
        );
        assert_eq!(
            store.get("s4").workspace_path.as_deref(),
            Some(root.join("from-x").as_path()),
            "目录边界:sibling 前缀不得误命中"
        );

        // 权威 sidecar 同步改写(s1);ACP 会话 s2 无 sidecar。bound_at 保留
        // 首次绑定时间,不被后面的索引补写段以 now 回填覆盖(评审 #463 minor)。
        let sidecar = read_code_session_sidecar(&store.path, "s1").unwrap();
        assert_eq!(sidecar.workspace_path.as_deref(), Some(to.as_path()));
        assert_eq!(sidecar.bound_at, bound_at_before);

        // 幂等:再跑一遍 from→to 无命中。
        assert!(
            store
                .rebind_workspace_prefix(&from, &to)
                .unwrap()
                .is_empty()
        );

        // 孤儿 sidecar(索引无记录)也会被改写,重启恢复不会复活旧目录。
        persist_code_session_sidecar(
            &code_session_sidecar_path(&store.path, "orphan"),
            &CodeSessionSidecar {
                version: CODE_SESSION_SIDECAR_VERSION,
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: Some(from.join("deep")),
                bound_at: None,
                workspace_roots: Vec::new(),
            },
        )
        .unwrap();
        // 候选集同口径含索引外孤儿(评审 #463 M6):栅栏不再漏保孤儿。
        let matched = store.sessions_under_workspace(&root.join("from"));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].0, "orphan");
        assert_eq!(matched[0].1, from.join("deep"));
        let affected = store
            .rebind_workspace_prefix(&root.join("from"), &root.join("to2"))
            .unwrap();
        fs::create_dir_all(root.join("to2")).unwrap();
        // 上一步 to 已改走;此轮 from 无索引命中,但孤儿 sidecar 命中。
        assert!(affected.iter().any(|(sid, _)| sid == "orphan"));
        let orphan = read_code_session_sidecar(&store.path, "orphan").unwrap();
        assert_eq!(
            orphan.workspace_path.as_deref(),
            Some(root.join("to2").join("deep").as_path())
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn recovered_acp_record_is_persisted_atomically() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-recovery-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .restore_missing_acp_record(
                "session-1",
                SessionAgentRecord {
                    backend: AgentBackend::ClaudeAcp,
                    acp_session_id: Some("acp-session-1".to_string()),
                    acp_model_id: Some("gpt-test".to_string()),
                    acp_mode_id: Some("agent".to_string()),
                    acp_config_values: HashMap::from([(
                        "reasoning_effort".to_string(),
                        "high".to_string(),
                    )]),
                    workspace_kind: CodexWorkspaceKind::Project,
                    workspace_path: Some(root.clone()),
                    mode: SessionMode::Plain,
                workspace_roots: Vec::new(),
            },
            )
            .unwrap();

        let persisted: AgentStoreFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let recovered = &persisted.sessions["session-1"];
        assert_eq!(recovered.backend, AgentBackend::ClaudeAcp);
        assert_eq!(recovered.acp_session_id.as_deref(), Some("acp-session-1"));
        assert_eq!(recovered.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(recovered.workspace_path.as_deref(), Some(root.as_path()));
        assert_eq!(
            recovered.acp_config_values.get("reasoning_effort"),
            Some(&"high".to_string())
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn codex_mode_is_persisted_with_the_session_record() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-mode-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_mode("session-1", Some("agent-full-access".to_string()))
            .unwrap();

        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            value["sessions"]["session-1"]["acp_mode_id"],
            "agent-full-access"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn generic_acp_config_is_persisted_with_the_session_record() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-acp-config-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_config_value("session-1", "reasoning_effort", "high")
            .unwrap();

        let persisted: AgentStoreFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            persisted.sessions["session-1"].acp_config_values["reasoning_effort"],
            "high"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn acp_defaults_are_isolated_by_agent_and_not_overwritten_by_migration() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-acp-defaults-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("acp-agent-defaults.json");
        let store = AcpConfigDefaultsStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set(AgentBackend::CodexAcp, "mode", "agent-full-access")
            .unwrap();
        store
            .set(AgentBackend::ClaudeAcp, "mode", "default")
            .unwrap();
        assert!(
            !store
                .set_all_if_absent(
                    AgentBackend::CodexAcp,
                    HashMap::from([("mode".to_string(), "agent".to_string())]),
                )
                .unwrap()
        );

        let persisted: AcpConfigDefaultsFile =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            persisted.agents[&AgentBackend::CodexAcp]["mode"],
            "agent-full-access"
        );
        assert_eq!(
            persisted.agents[&AgentBackend::ClaudeAcp]["mode"],
            "default"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_binding_writes_authoritative_sidecar() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-write-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        let sidecar = read_code_session_sidecar(store.path(), "session-1")
            .expect("sidecar should exist after binding");
        assert_eq!(sidecar.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        assert!(sidecar.bound_at.is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_sidecar_recovers_missing_index_record() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-recover-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        // 模拟辅助索引丢失：清空记录并从磁盘重建（empty store）。
        let recovered_store = SessionAgentStore {
            path: store.path().to_path_buf(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert!(!recovered_store.is_code_session("session-1"));
        let sidecar = read_code_session_sidecar(recovered_store.path(), "session-1").unwrap();
        recovered_store
            .restore_missing_code_session_record("session-1", sidecar)
            .unwrap();
        let record = recovered_store.get("session-1");
        assert!(record.mode.is_code());
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(record.workspace_path.as_deref(), Some(root.as_path()));
        // 恢复持久化回索引文件，后续读取不再依赖 sidecar。
        let persisted: AgentStoreFile =
            serde_json::from_slice(&fs::read(recovered_store.path()).unwrap()).unwrap();
        assert!(persisted.sessions["session-1"].mode.is_code());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_sidecar_restore_rejects_acp_owned_session() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-acp-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        let sidecar = read_code_session_sidecar(store.path(), "session-1").unwrap();
        // ACP 会话已占用该 session：恢复必须拒绝，不能覆盖。
        store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                Vec::new(),
            )
            .unwrap();
        assert!(
            store
                .restore_missing_code_session_record("session-1", sidecar)
                .is_err()
        );
        assert!(store.get("session-1").backend.is_acp());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn remove_cleans_up_code_native_sidecar() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-remove-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None, Vec::new())
            .unwrap();
        assert!(read_code_session_sidecar(store.path(), "session-1").is_some());
        store.remove("session-1").unwrap();
        assert!(read_code_session_sidecar(store.path(), "session-1").is_none());
        assert!(!store.is_code_session("session-1"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_sidecar_rejects_malformed_kind_path_combination() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-malformed-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        // 项目 kind 但缺路径：恢复必须拒绝。
        let missing_path = CodeSessionSidecar {
            version: CODE_SESSION_SIDECAR_VERSION,
            workspace_kind: CodexWorkspaceKind::Project,
            workspace_path: None,
            bound_at: None,
                workspace_roots: Vec::new(),
            };
        assert!(
            store
                .restore_missing_code_session_record("session-1", missing_path)
                .is_err()
        );
        // 临时 kind 但带路径：恢复必须拒绝。
        let with_path = CodeSessionSidecar {
            version: CODE_SESSION_SIDECAR_VERSION,
            workspace_kind: CodexWorkspaceKind::Temporary,
            workspace_path: Some(root.clone()),
            bound_at: None,
                workspace_roots: Vec::new(),
            };
        assert!(
            store
                .restore_missing_code_session_record("session-2", with_path)
                .is_err()
        );
        assert!(!store.is_code_session("session-1"));
        assert!(!store.is_code_session("session-2"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_restore_reports_real_recovery_only() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-restore-count-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        let sidecar = read_code_session_sidecar(store.path(), "session-1").unwrap();
        // 模拟辅助索引丢失后的首次恢复：真实恢复，返回 true。
        let recovered_store = SessionAgentStore {
            path: store.path().to_path_buf(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert!(
            recovered_store
                .restore_missing_code_session_record("session-1", sidecar.clone())
                .unwrap()
        );
        // 索引已完好：再次调用是早退，不得被误计为恢复信号。
        assert!(
            !recovered_store
                .restore_missing_code_session_record("session-1", sidecar)
                .unwrap()
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_code_native_sidecar_is_backfilled_from_index() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-backfill-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        // 非代码会话不参与回填。
        store
            .set_acp_workspace(
                "session-2",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
                Vec::new(),
            )
            .unwrap();
        // 模拟存量会话/绑定时写失败：索引记录 code_session=true 但 sidecar 缺失。
        fs::remove_file(code_session_sidecar_path(store.path(), "session-1")).unwrap();
        assert_eq!(store.backfill_missing_code_session_sidecars(), 1);
        let sidecar = read_code_session_sidecar(store.path(), "session-1")
            .expect("sidecar should be backfilled");
        assert_eq!(sidecar.version, CODE_SESSION_SIDECAR_VERSION);
        assert_eq!(sidecar.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        // 幂等：sidecar 完好、非代码会话都不补写。
        assert_eq!(store.backfill_missing_code_session_sidecars(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_native_sidecar_rejects_newer_schema_version() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-sidecar-version-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        // 写入高于当前支持版本的 sidecar：拒读并按缺失处理，不能静默按 v1 解析。
        let future = CodeSessionSidecar {
            version: CODE_SESSION_SIDECAR_VERSION + 1,
            workspace_kind: CodexWorkspaceKind::Project,
            workspace_path: Some(root.join("future-workspace")),
            bound_at: None,
                workspace_roots: Vec::new(),
            };
        fs::write(
            code_session_sidecar_path(store.path(), "session-1"),
            serde_json::to_vec(&future).unwrap(),
        )
        .unwrap();
        assert!(read_code_session_sidecar(store.path(), "session-1").is_none());
        // 按缺失处理 → 回填自愈按索引重写为当前版本。
        assert_eq!(store.backfill_missing_code_session_sidecars(), 1);
        let sidecar = read_code_session_sidecar(store.path(), "session-1").unwrap();
        assert_eq!(sidecar.version, CODE_SESSION_SIDECAR_VERSION);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn failed_index_persist_keeps_code_native_sidecar() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-persist-order-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        let store = SessionAgentStore {
            path: path.clone(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()), Vec::new())
            .unwrap();
        assert!(read_code_session_sidecar(&path, "session-1").is_some());
        // 让索引 persist 必失败：索引路径被同名目录占用，rename 无法覆盖。
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(
            store
                .set_acp_workspace(
                    "session-1",
                    AgentBackend::CodexAcp,
                    CodexWorkspaceKind::Temporary,
                    None,
                Vec::new(),
            )
                .is_err()
        );
        // persist 先失败则 sidecar 不得先删，与磁盘索引（仍是绑定时的内容）保持一致。
        let sidecar = read_code_session_sidecar(&path, "session-1")
            .expect("sidecar must survive failed index persist");
        assert_eq!(sidecar.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn code_session_roots_snapshot_persist_restore_and_rebind() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-roots-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let project_dir = root.join("proj");
        let shared = project_dir.join("shared");
        let outside = root.join("outside");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        let roots = vec![project_dir.clone(), shared.clone(), outside.clone()];
        store
            .bind_code_native_session(
                "code-1",
                CodexWorkspaceKind::Project,
                Some(project_dir.clone()),
                roots.clone(),
            )
            .unwrap();
        // 索引记录与权威 sidecar 都携带快照;读取点透出。
        assert_eq!(store.session_workspace_roots("code-1"), roots);
        let sidecar = read_code_session_sidecar(&store.path, "code-1").expect("sidecar");
        assert_eq!(sidecar.workspace_roots, roots);

        // 索引丢失 → 从 sidecar 恢复,快照不丢。
        store.records.write().clear();
        store
            .restore_missing_code_session_record("code-1", sidecar)
            .expect("restore");
        assert_eq!(store.session_workspace_roots("code-1"), roots);

        // 重绑定:proj 前缀下的根平移,外部根原样。
        let moved = root.join("moved");
        fs::create_dir_all(&moved).unwrap();
        store
            .rebind_workspace_prefix(&project_dir, &moved)
            .expect("rebind");
        assert_eq!(
            store.session_workspace_roots("code-1"),
            vec![moved.clone(), moved.join("shared"), outside.clone()]
        );
        let sidecar = read_code_session_sidecar(&store.path, "code-1").expect("sidecar after");
        assert_eq!(sidecar.workspace_roots[0], moved);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn acp_workspace_carries_roots_and_temporary_stays_empty() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-acp-roots-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .set_acp_workspace(
                "acp-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(root.clone()),
                vec![root.clone()],
            )
            .unwrap();
        assert_eq!(store.session_workspace_roots("acp-1"), vec![root.clone()]);
        // 临时会话恒空(单根语义),即使误传也不落。
        store
            .set_acp_workspace(
                "acp-2",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
                vec![root.clone()],
            )
            .unwrap();
        assert_eq!(store.session_workspace_roots("acp-2"), Vec::<PathBuf>::new());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn set_session_workspace_roots_rewrites_record_and_sidecar() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-align-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let project_dir = root.join("proj");
        fs::create_dir_all(&project_dir).unwrap();
        let store = SessionAgentStore {
            path: root.join("session-agents.json"),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        store
            .bind_code_native_session(
                "code-1",
                CodexWorkspaceKind::Project,
                Some(project_dir.clone()),
                vec![project_dir.clone()],
            )
            .unwrap();
        // 原生代码会话:索引 + 权威 sidecar 双写,bound_at 保留。
        let bound_at = read_code_session_sidecar(&store.path, "code-1").unwrap().bound_at;
        let extra = root.join("extra");
        let next = vec![project_dir.clone(), extra.clone()];
        assert!(store.set_session_workspace_roots("code-1", next.clone()).unwrap());
        assert_eq!(store.session_workspace_roots("code-1"), next);
        let sidecar = read_code_session_sidecar(&store.path, "code-1").unwrap();
        assert_eq!(sidecar.workspace_roots, next);
        assert_eq!(sidecar.bound_at, bound_at, "对齐不改写首次绑定时间");

        // 临时会话/未知会话 = Ok(false),不写盘。
        assert!(!store.set_session_workspace_roots("temp-unknown", next).unwrap());
        fs::remove_dir_all(&root).unwrap();
    }
}
