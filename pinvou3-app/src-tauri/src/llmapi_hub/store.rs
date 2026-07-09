use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::models::{
    DeviceBindingStatus, LlmApiAdminOverviewItem, LlmApiBinding, LlmApiError, LlmApiErrorCode,
    ProvisioningStatus,
};

pub trait LlmApiBindingStore {
    fn get_binding(
        &self,
        pinvou_user_id: &str,
        device_binding_id: &str,
    ) -> Result<Option<LlmApiBinding>, LlmApiError>;
    fn upsert_binding(&self, binding: LlmApiBinding) -> Result<(), LlmApiError>;
    fn list_bindings(&self) -> Result<Vec<LlmApiBinding>, LlmApiError>;
    fn set_enabled(
        &self,
        pinvou_user_id: &str,
        enabled: bool,
    ) -> Result<LlmApiBinding, LlmApiError>;
}

#[derive(Debug, Clone)]
pub struct FileLlmApiBindingStore {
    path: PathBuf,
}

impl Default for FileLlmApiBindingStore {
    fn default() -> Self {
        Self::new(default_bindings_path())
    }
}

impl FileLlmApiBindingStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn load_db(&self) -> Result<BindingDb, LlmApiError> {
        if !self.path.exists() {
            return Ok(BindingDb::default());
        }
        let bytes = std::fs::read(&self.path).map_err(|err| {
            LlmApiError::new(
                LlmApiErrorCode::Unavailable,
                format!("读取 LLM API Hub 绑定状态失败: {err}"),
                true,
            )
        })?;
        serde_json::from_slice(&bytes).map_err(|err| {
            LlmApiError::new(
                LlmApiErrorCode::Unavailable,
                format!("解析 LLM API Hub 绑定状态失败: {err}"),
                true,
            )
        })
    }

    fn save_db(&self, db: &BindingDb) -> Result<(), LlmApiError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                LlmApiError::new(
                    LlmApiErrorCode::Unavailable,
                    format!("创建 LLM API Hub 状态目录失败: {err}"),
                    true,
                )
            })?;
        }
        let bytes = serde_json::to_vec_pretty(db).map_err(|err| {
            LlmApiError::new(
                LlmApiErrorCode::Unavailable,
                format!("序列化 LLM API Hub 状态失败: {err}"),
                true,
            )
        })?;
        std::fs::write(&self.path, bytes).map_err(|err| {
            LlmApiError::new(
                LlmApiErrorCode::Unavailable,
                format!("保存 LLM API Hub 状态失败: {err}"),
                true,
            )
        })
    }
}

impl LlmApiBindingStore for FileLlmApiBindingStore {
    fn get_binding(
        &self,
        pinvou_user_id: &str,
        device_binding_id: &str,
    ) -> Result<Option<LlmApiBinding>, LlmApiError> {
        let db = self.load_db()?;
        Ok(db
            .bindings
            .get(&binding_key(pinvou_user_id, device_binding_id))
            .cloned())
    }

    fn upsert_binding(&self, binding: LlmApiBinding) -> Result<(), LlmApiError> {
        let mut db = self.load_db()?;
        db.bindings.insert(
            binding_key(&binding.pinvou_user_id, &binding.device_binding_id),
            binding,
        );
        self.save_db(&db)
    }

    fn list_bindings(&self) -> Result<Vec<LlmApiBinding>, LlmApiError> {
        let db = self.load_db()?;
        Ok(db.bindings.into_values().collect())
    }

    fn set_enabled(
        &self,
        pinvou_user_id: &str,
        enabled: bool,
    ) -> Result<LlmApiBinding, LlmApiError> {
        let mut db = self.load_db()?;
        let key = db
            .bindings
            .keys()
            .find(|key| key.starts_with(&format!("{pinvou_user_id}:")))
            .cloned()
            .ok_or_else(|| {
                LlmApiError::new(
                    LlmApiErrorCode::UserNotFound,
                    "未找到指定用户的 LLM API Hub 绑定",
                    false,
                )
            })?;
        let binding = db.bindings.get_mut(&key).expect("binding key from map");
        binding.enabled = enabled;
        binding.provisioning_status = if enabled {
            ProvisioningStatus::Ready
        } else {
            ProvisioningStatus::Disabled
        };
        binding.updated_at = Utc::now();
        let updated = binding.clone();
        self.save_db(&db)?;
        Ok(updated)
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryLlmApiBindingStore {
    bindings: Arc<Mutex<HashMap<String, LlmApiBinding>>>,
}

impl LlmApiBindingStore for MemoryLlmApiBindingStore {
    fn get_binding(
        &self,
        pinvou_user_id: &str,
        device_binding_id: &str,
    ) -> Result<Option<LlmApiBinding>, LlmApiError> {
        Ok(self
            .bindings
            .lock()
            .expect("memory llmapi binding lock")
            .get(&binding_key(pinvou_user_id, device_binding_id))
            .cloned())
    }

    fn upsert_binding(&self, binding: LlmApiBinding) -> Result<(), LlmApiError> {
        self.bindings
            .lock()
            .expect("memory llmapi binding lock")
            .insert(
                binding_key(&binding.pinvou_user_id, &binding.device_binding_id),
                binding,
            );
        Ok(())
    }

    fn list_bindings(&self) -> Result<Vec<LlmApiBinding>, LlmApiError> {
        Ok(self
            .bindings
            .lock()
            .expect("memory llmapi binding lock")
            .values()
            .cloned()
            .collect())
    }

    fn set_enabled(
        &self,
        pinvou_user_id: &str,
        enabled: bool,
    ) -> Result<LlmApiBinding, LlmApiError> {
        let mut bindings = self.bindings.lock().expect("memory llmapi binding lock");
        let key = bindings
            .keys()
            .find(|key| key.starts_with(&format!("{pinvou_user_id}:")))
            .cloned()
            .ok_or_else(|| {
                LlmApiError::new(
                    LlmApiErrorCode::UserNotFound,
                    "未找到指定用户的 LLM API Hub 绑定",
                    false,
                )
            })?;
        let binding = bindings.get_mut(&key).expect("binding key from map");
        binding.enabled = enabled;
        binding.provisioning_status = if enabled {
            ProvisioningStatus::Ready
        } else {
            ProvisioningStatus::Disabled
        };
        binding.updated_at = Utc::now();
        Ok(binding.clone())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BindingDb {
    #[serde(default)]
    bindings: HashMap<String, LlmApiBinding>,
}

pub fn default_bindings_path() -> PathBuf {
    crate::bridge::paths::pinvou3_home()
        .join("llmapi-hub")
        .join("bindings.json")
}

pub fn binding_key(pinvou_user_id: &str, device_binding_id: &str) -> String {
    format!("{}:{}", pinvou_user_id.trim(), device_binding_id.trim())
}

pub fn admin_overview_items(bindings: Vec<LlmApiBinding>) -> Vec<LlmApiAdminOverviewItem> {
    bindings
        .into_iter()
        .map(|binding| LlmApiAdminOverviewItem {
            pinvou_user_id: binding.pinvou_user_id,
            device_binding_status: DeviceBindingStatus::Bound,
            enabled: binding.enabled,
            provisioning_status: binding.provisioning_status,
            newapi_user_id: binding.newapi_user_id,
            newapi_token_id: binding.newapi_token_id,
            quota_used_tokens: binding.usage.used_tokens,
            quota_limit_tokens: binding.usage.limit_tokens,
            last_error_code: binding.last_error_code,
            last_error_message: binding.last_error_message,
            updated_at: binding.updated_at,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llmapi_hub::models::{LlmApiIdentity, LlmApiPolicy};

    #[test]
    fn memory_store_uses_user_and_device_as_unique_key() {
        let store = MemoryLlmApiBindingStore::default();
        let identity = LlmApiIdentity {
            pinvou_user_id: "u_1".to_string(),
            device_binding_id: "dev_a".to_string(),
            bios_sn_hash: "hash_a".to_string(),
        };
        let mut other_identity = identity.clone();
        other_identity.device_binding_id = "dev_b".to_string();

        store
            .upsert_binding(LlmApiBinding::new(&identity, LlmApiPolicy::default()))
            .unwrap();
        store
            .upsert_binding(LlmApiBinding::new(&other_identity, LlmApiPolicy::default()))
            .unwrap();

        assert!(store.get_binding("u_1", "dev_a").unwrap().is_some());
        assert!(store.get_binding("u_1", "dev_b").unwrap().is_some());
        assert_eq!(store.list_bindings().unwrap().len(), 2);
    }

    #[test]
    fn set_enabled_persists_status() {
        let store = MemoryLlmApiBindingStore::default();
        let identity = LlmApiIdentity {
            pinvou_user_id: "u_1".to_string(),
            device_binding_id: "dev_a".to_string(),
            bios_sn_hash: "hash_a".to_string(),
        };
        store
            .upsert_binding(LlmApiBinding::new(&identity, LlmApiPolicy::default()))
            .unwrap();

        let disabled = store.set_enabled("u_1", false).unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.provisioning_status, ProvisioningStatus::Disabled);
    }
}
