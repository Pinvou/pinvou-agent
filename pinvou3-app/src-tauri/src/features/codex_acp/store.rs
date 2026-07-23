use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

const STORE_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentBackend {
    #[default]
    Deepseek,
    CodexAcp,
}

impl AgentBackend {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("deepseek") {
            "deepseek" => Ok(Self::Deepseek),
            "codex-acp" | "codex" => Ok(Self::CodexAcp),
            other => anyhow::bail!("不支持的 Agent 后端: {other}"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::CodexAcp => "codex-acp",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexWorkspaceKind {
    #[default]
    Temporary,
    Project,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionAgentRecord {
    #[serde(default)]
    pub backend: AgentBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_model_id: Option<String>,
    /// 用户明确选择的 Codex 权限模式。ACP Agent 在 new/load 时会恢复默认
    /// `agent`，所以 Pinvou 必须在运行时就绪前重新应用该期望值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_mode_id: Option<String>,
    /// Codex 的执行目录类型。旧记录没有该字段时按临时会话兼容。
    #[serde(default)]
    pub workspace_kind: CodexWorkspaceKind,
    /// 项目会话保存创建时选定的绝对目录；临时会话目录由 session id 推导。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<PathBuf>,
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

    /// Codex ACP 是可选能力，它的辅助索引损坏时不能阻断 Pinvou 主程序启动。
    ///
    /// 这里保留原始文件供排障，不主动覆盖；只有用户后续实际创建或更新 Codex
    /// 会话时，`persist` 才会用新的有效内容替换它。
    pub fn load_or_empty() -> Self {
        match Self::load() {
            Ok(store) => store,
            Err(error) => {
                let path = crate::platform::paths::pinvou3_home().join("session-agents.json");
                eprintln!(
                    "[pinvou3-app] Codex ACP session index unavailable, starting empty: {error:#}"
                );
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

    pub fn get(&self, session_id: &str) -> SessionAgentRecord {
        self.records
            .read()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_backend(&self, session_id: &str, backend: AgentBackend) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            if record.backend != backend {
                record.backend = backend;
                record.acp_session_id = None;
                record.acp_model_id = None;
                record.acp_mode_id = None;
                record.workspace_kind = CodexWorkspaceKind::Temporary;
                record.workspace_path = None;
            }
        }
        self.persist()
    }

    /// 在 Codex 会话创建时永久绑定执行目录。
    ///
    /// ACP session 一旦建立就不允许换目录，避免同一个 Codex 上下文跨项目漂移。
    pub fn set_codex_workspace(
        &self,
        session_id: &str,
        kind: CodexWorkspaceKind,
        workspace_path: Option<PathBuf>,
    ) -> Result<()> {
        if kind == CodexWorkspaceKind::Project && workspace_path.is_none() {
            anyhow::bail!("项目会话缺少 Codex 工作目录");
        }
        if kind == CodexWorkspaceKind::Temporary && workspace_path.is_some() {
            anyhow::bail!("临时会话不能保存项目工作目录");
        }
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            if record.acp_session_id.is_some()
                && (record.workspace_kind != kind || record.workspace_path != workspace_path)
            {
                anyhow::bail!("Codex 会话已开始，不能更换工作目录；请新建会话");
            }
            record.backend = AgentBackend::CodexAcp;
            record.workspace_kind = kind;
            record.workspace_path = workspace_path;
        }
        self.persist()
    }

    pub fn set_acp_session(
        &self,
        session_id: &str,
        acp_session_id: String,
        model_id: Option<String>,
    ) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record.backend = AgentBackend::CodexAcp;
            record.acp_session_id = Some(acp_session_id);
            record.acp_model_id = model_id;
        }
        self.persist()
    }

    pub fn set_acp_model(&self, session_id: &str, model_id: Option<String>) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record.acp_model_id = model_id;
        }
        self.persist()
    }

    pub fn set_acp_mode(&self, session_id: &str, mode_id: Option<String>) -> Result<()> {
        {
            let mut records = self.records.write();
            let record = records.entry(session_id.to_string()).or_default();
            record.acp_mode_id = mode_id;
        }
        self.persist()
    }

    pub fn remove(&self, session_id: &str) -> Result<()> {
        self.records.write().remove(session_id);
        self.persist()
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

pub fn validate_codex_project_workspace(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("请选择 Codex 项目目录");
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Codex 项目目录不存在或不可访问: {}", path.display()))?;
    if !canonical.is_dir() {
        anyhow::bail!("Codex 工作目录必须是文件夹: {}", canonical.display());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_aliases_are_stable() {
        assert_eq!(AgentBackend::parse(None).unwrap(), AgentBackend::Deepseek);
        assert_eq!(
            AgentBackend::parse(Some("codex")).unwrap(),
            AgentBackend::CodexAcp
        );
        assert_eq!(AgentBackend::CodexAcp.as_str(), "codex-acp");
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
    }

    #[test]
    fn project_workspace_must_exist_and_be_a_directory() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-workspace-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            validate_codex_project_workspace(&root).unwrap(),
            root.canonicalize().unwrap()
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
            .set_codex_workspace("session-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        store
            .set_acp_session("session-1", "acp-1".to_string(), None)
            .unwrap();
        assert!(store
            .set_codex_workspace("session-1", CodexWorkspaceKind::Temporary, None)
            .is_err());
        assert_eq!(
            store.get("session-1").workspace_path.as_deref(),
            Some(root.as_path())
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
}
