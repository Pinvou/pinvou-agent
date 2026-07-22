use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

const STORE_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionAgentRecord {
    #[serde(default)]
    pub backend: AgentBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_model_id: Option<String>,
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
        let path = crate::bridge::paths::pinvou3_home().join("session-agents.json");
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
            }
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
}
