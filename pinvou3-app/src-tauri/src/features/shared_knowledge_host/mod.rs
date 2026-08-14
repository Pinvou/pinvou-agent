//! PINVOU 托管共享知识库宿主的稳定业务接口。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const LOCAL_ENDPOINT: &str = "https://127.0.0.1:3210";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedKnowledgeHostStatus {
    pub supported: bool,
    pub installed: bool,
    pub running: bool,
    pub endpoint: String,
    pub service_version: Option<String>,
    pub app_version: String,
    pub upgrade_available: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostOwnerClaim {
    pub server_id: String,
    pub device_id: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostBackupResult {
    pub manifest: serde_json::Value,
    pub recovery_code: String,
}

#[derive(Debug, Clone)]
pub struct HostRestoreResult {
    pub manifest: serde_json::Value,
    pub owner_claim: Option<HostOwnerClaim>,
}

#[derive(Debug, Clone)]
pub struct PackagedHostResources {
    pub helper: PathBuf,
    pub server: PathBuf,
}

pub fn packaged_resources(resource_dir: &Path) -> PackagedHostResources {
    let root = resource_dir.join("runtime").join("knowledge-host");
    PackagedHostResources {
        helper: root.join("pinvou-knowledge-host-helper"),
        server: root.join("pinvou-knowledge-server"),
    }
}

mod platform;

pub use platform::{
    backup_host, consume_owner_claim, install_or_upgrade, lan_endpoints, recover_owner,
    remove_host, restore_host, set_owner_device, status,
};
