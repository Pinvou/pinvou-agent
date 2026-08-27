use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use pinvou_runtime_api::{ApprovalProfile, LogicalSessionId};
use serde::{Deserialize, Serialize};

use crate::session_store::{SessionStoreError, atomic_write_json, stable_key};

const WORKSPACE_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspacePreferences {
    pub runtime: Option<String>,
    pub model_by_runtime: BTreeMap<String, String>,
    #[serde(default)]
    pub reasoning_level_by_runtime: BTreeMap<String, String>,
    pub approval_profile: ApprovalProfile,
    pub recent_session: Option<LogicalSessionId>,
}

impl Default for WorkspacePreferences {
    fn default() -> Self {
        Self {
            runtime: None,
            model_by_runtime: BTreeMap::new(),
            reasoning_level_by_runtime: BTreeMap::new(),
            approval_profile: ApprovalProfile::Request,
            recent_session: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredPreferences {
    schema_version: u16,
    preferences: WorkspacePreferences,
}

#[derive(Clone, Debug)]
pub struct WorkspaceStore {
    root: PathBuf,
}

impl WorkspaceStore {
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self, SessionStoreError> {
        let root = data_root.as_ref().join("workspaces");
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn load(
        &self,
        workspace: &Path,
    ) -> Result<Option<WorkspacePreferences>, SessionStoreError> {
        let path = self.preference_path(workspace)?;
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stored: StoredPreferences = serde_json::from_slice(&bytes)?;
        if stored.schema_version != WORKSPACE_SCHEMA_VERSION {
            return Err(SessionStoreError::Corrupt("unsupported workspace schema"));
        }
        Ok(Some(stored.preferences))
    }

    pub fn save(
        &self,
        workspace: &Path,
        preferences: &WorkspacePreferences,
    ) -> Result<(), SessionStoreError> {
        atomic_write_json(
            &self.preference_path(workspace)?,
            &StoredPreferences {
                schema_version: WORKSPACE_SCHEMA_VERSION,
                preferences: preferences.clone(),
            },
        )
    }

    fn preference_path(&self, workspace: &Path) -> Result<PathBuf, SessionStoreError> {
        Ok(self
            .root
            .join(self.workspace_key(workspace)?)
            .join("preferences.json"))
    }

    pub fn workspace_key(&self, workspace: &Path) -> Result<String, SessionStoreError> {
        let workspace = workspace.canonicalize()?;
        let mut normalized = workspace.to_string_lossy().replace('\\', "/");
        if cfg!(windows) {
            normalized.make_ascii_lowercase();
        }
        Ok(stable_key(normalized.as_bytes()))
    }
}
