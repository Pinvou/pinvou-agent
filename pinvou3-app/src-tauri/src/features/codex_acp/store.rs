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
        .unwrap_or_else(|| crate::platform::paths::sessions_root())
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
) -> bool {
    let path = code_session_sidecar_path(store_path, session_id);
    let sidecar = CodeSessionSidecar {
        version: CODE_SESSION_SIDECAR_VERSION,
        workspace_kind: kind,
        workspace_path,
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
            record.mode = SessionMode::Code;
        }
        self.persist()?;
        // 权威 sidecar：辅助索引损坏/丢失后据此恢复原生代码会话类型与项目绑定。
        // 写失败已逐条记日志；缺失的 sidecar 由启动时回填自愈补写。
        write_code_session_sidecar(&self.path, session_id, kind, workspace_path);
        Ok(())
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
            )
            .unwrap();
        store
            .set_acp_session("session-1", "acp-1".to_string(), None, HashMap::new())
            .unwrap();
        assert!(store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
            )
            .is_err());
        assert!(store
            .set_acp_workspace(
                "session-1",
                AgentBackend::ClaudeAcp,
                CodexWorkspaceKind::Project,
                Some(root.clone()),
            )
            .is_err());
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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None)
            .unwrap();

        let record = store.get("session-1");
        assert_eq!(record.backend, AgentBackend::Deepseek);
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Temporary);
        assert_eq!(record.workspace_path, None);
        assert!(record.mode.is_code());
        assert!(store.is_code_session("session-1"));
        // 原生绑定不需要 Agent 上下文，同值重复绑定保持幂等。
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None)
            .unwrap();
        assert!(store.is_code_session("session-1"));

        let persisted: AgentStoreFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let record = persisted.sessions.get("session-1").unwrap();
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
            )
            .unwrap();
        store
            .set_acp_session("session-1", "acp-1".to_string(), None, HashMap::new())
            .unwrap();
        assert!(store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None)
            .is_err());
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
        assert!(store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, None)
            .is_err());
        assert!(store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Temporary,
                Some(root.clone()),
            )
            .is_err());
        assert!(!store.is_code_session("session-1"));

        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        let record = store.get("session-1");
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(record.workspace_path.as_deref(), Some(root.as_path()));
        assert!(record.mode.is_code());

        // 已绑定的代码会话不可改绑工作区；同值重复绑定幂等。
        assert!(store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None)
            .is_err());
        assert!(store
            .bind_code_native_session(
                "session-1",
                CodexWorkspaceKind::Project,
                Some(root.join("other")),
            )
            .is_err());
        store
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        let record = store.get("session-1");
        assert_eq!(record.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(record.workspace_path.as_deref(), Some(root.as_path()));
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
                },
            )
            .unwrap();

        let persisted: AgentStoreFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let recovered = persisted.sessions.get("session-1").unwrap();
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
        assert!(!store
            .set_all_if_absent(
                AgentBackend::CodexAcp,
                HashMap::from([("mode".to_string(), "agent".to_string())]),
            )
            .unwrap());

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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        let sidecar = read_code_session_sidecar(&store.path(), "session-1")
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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        let sidecar = read_code_session_sidecar(store.path(), "session-1").unwrap();
        // ACP 会话已占用该 session：恢复必须拒绝，不能覆盖。
        store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(root.clone()),
            )
            .unwrap();
        assert!(store
            .restore_missing_code_session_record("session-1", sidecar)
            .is_err());
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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Temporary, None)
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
        };
        assert!(store
            .restore_missing_code_session_record("session-1", missing_path)
            .is_err());
        // 临时 kind 但带路径：恢复必须拒绝。
        let with_path = CodeSessionSidecar {
            version: CODE_SESSION_SIDECAR_VERSION,
            workspace_kind: CodexWorkspaceKind::Temporary,
            workspace_path: Some(root.clone()),
            bound_at: None,
        };
        assert!(store
            .restore_missing_code_session_record("session-2", with_path)
            .is_err());
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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        let sidecar = read_code_session_sidecar(store.path(), "session-1").unwrap();
        // 模拟辅助索引丢失后的首次恢复：真实恢复，返回 true。
        let recovered_store = SessionAgentStore {
            path: store.path().to_path_buf(),
            records: Arc::new(RwLock::new(HashMap::new())),
        };
        assert!(recovered_store
            .restore_missing_code_session_record("session-1", sidecar.clone())
            .unwrap());
        // 索引已完好：再次调用是早退，不得被误计为恢复信号。
        assert!(!recovered_store
            .restore_missing_code_session_record("session-1", sidecar)
            .unwrap());
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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        // 非代码会话不参与回填。
        store
            .set_acp_workspace(
                "session-2",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        // 写入高于当前支持版本的 sidecar：拒读并按缺失处理，不能静默按 v1 解析。
        let future = CodeSessionSidecar {
            version: CODE_SESSION_SIDECAR_VERSION + 1,
            workspace_kind: CodexWorkspaceKind::Project,
            workspace_path: Some(root.join("future-workspace")),
            bound_at: None,
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
            .bind_code_native_session("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        assert!(read_code_session_sidecar(&path, "session-1").is_some());
        // 让索引 persist 必失败：索引路径被同名目录占用，rename 无法覆盖。
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(store
            .set_acp_workspace(
                "session-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
            )
            .is_err());
        // persist 先失败则 sidecar 不得先删，与磁盘索引（仍是绑定时的内容）保持一致。
        let sidecar = read_code_session_sidecar(&path, "session-1")
            .expect("sidecar must survive failed index persist");
        assert_eq!(sidecar.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(sidecar.workspace_path.as_deref(), Some(root.as_path()));
        fs::remove_dir_all(&root).unwrap();
    }
}
