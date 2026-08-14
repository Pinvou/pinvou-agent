//! 非 Linux 平台只提供加入能力，宿主生命周期操作显式返回不支持。

use std::path::PathBuf;

use pinvou_knowledge::model::DeviceGrant;

use super::super::{
    HostOwnerClaim, HostRestoreResult, PackagedHostResources, SharedKnowledgeHostStatus,
    LOCAL_ENDPOINT,
};

pub async fn status() -> SharedKnowledgeHostStatus {
    SharedKnowledgeHostStatus {
        supported: false,
        installed: false,
        running: false,
        endpoint: LOCAL_ENDPOINT.to_string(),
        service_version: None,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        upgrade_available: false,
        client_outdated: false,
    }
}

pub async fn install_or_upgrade(
    _resources: PackagedHostResources,
    _model_dir: PathBuf,
    _upgrade: bool,
) -> Result<Option<HostOwnerClaim>, String> {
    Err("当前系统只能加入共享知识库，创建宿主首期支持 Linux".to_string())
}

pub async fn set_owner_device(
    _resources: PackagedHostResources,
    _device_id: String,
    _owner: bool,
) -> Result<DeviceGrant, String> {
    Err("所有者变更只能在 Linux 宿主本机执行".to_string())
}

pub async fn consume_owner_claim(
    _resources: PackagedHostResources,
) -> Result<HostOwnerClaim, String> {
    Err("本机所有者凭据只存在于 Linux 宿主".to_string())
}

pub async fn recover_owner(_resources: PackagedHostResources) -> Result<HostOwnerClaim, String> {
    Err("本机所有者恢复只支持 Linux 宿主".to_string())
}

pub async fn remove_host(
    _resources: PackagedHostResources,
    _delete_data: bool,
) -> Result<(), String> {
    Err("移除宿主只支持 Linux".to_string())
}

pub async fn backup_host(
    _resources: PackagedHostResources,
    _output: PathBuf,
    _local_recipient: String,
    _recovery_recipient: String,
) -> Result<serde_json::Value, String> {
    Err("备份宿主只支持 Linux".to_string())
}

pub async fn restore_host(
    _resources: PackagedHostResources,
    _input: PathBuf,
    _identity_file: PathBuf,
    _content_only: bool,
) -> Result<HostRestoreResult, String> {
    Err("恢复宿主只支持 Linux".to_string())
}

pub fn lan_endpoints() -> Vec<String> {
    Vec::new()
}
