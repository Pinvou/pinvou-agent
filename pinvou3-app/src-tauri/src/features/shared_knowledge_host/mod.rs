//! PINVOU 托管共享知识库宿主的稳定业务接口。

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use semver::Version;
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
    pub client_outdated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostVersionState {
    UpgradeAvailable,
    Current,
    ClientOutdated,
    Unknown,
}

pub(crate) fn compare_host_versions(
    app_version: &str,
    service_version: Option<&str>,
) -> HostVersionState {
    let Ok(app_version) = Version::parse(app_version) else {
        return HostVersionState::Unknown;
    };
    let Some(service_version) = service_version else {
        return HostVersionState::Unknown;
    };
    let Ok(service_version) = Version::parse(service_version) else {
        return HostVersionState::Unknown;
    };
    match app_version.cmp_precedence(&service_version) {
        Ordering::Greater => HostVersionState::UpgradeAvailable,
        Ordering::Equal => HostVersionState::Current,
        Ordering::Less => HostVersionState::ClientOutdated,
    }
}

pub(crate) fn ensure_host_install_allowed(
    status: &SharedKnowledgeHostStatus,
) -> Result<(), String> {
    if !status.client_outdated {
        return Ok(());
    }
    Err(format!(
        "当前 PINVOU 版本 {} 低于已安装的共享知识库服务 {}，已拒绝降级；请先升级 PINVOU",
        status.app_version,
        status.service_version.as_deref().unwrap_or("未知版本")
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn status(client_outdated: bool) -> SharedKnowledgeHostStatus {
        SharedKnowledgeHostStatus {
            supported: true,
            installed: true,
            running: true,
            endpoint: LOCAL_ENDPOINT.to_string(),
            service_version: Some("0.10.0".to_string()),
            app_version: "0.9.9".to_string(),
            upgrade_available: false,
            client_outdated,
        }
    }

    #[test]
    fn host_versions_follow_semver_precedence() {
        assert_eq!(
            compare_host_versions("0.10.0", Some("0.9.99")),
            HostVersionState::UpgradeAvailable
        );
        assert_eq!(
            compare_host_versions("0.8.1", Some("0.8.1")),
            HostVersionState::Current
        );
        assert_eq!(
            compare_host_versions("0.8.1-rc.2", Some("0.8.1")),
            HostVersionState::ClientOutdated
        );
        assert_eq!(
            compare_host_versions("0.8.1", Some("0.8.1-rc.2")),
            HostVersionState::UpgradeAvailable
        );
        assert_eq!(
            compare_host_versions("0.8.1+desktop", Some("0.8.1+service")),
            HostVersionState::Current
        );
    }

    #[test]
    fn unknown_versions_never_offer_an_upgrade_or_downgrade() {
        assert_eq!(
            compare_host_versions("0.8", Some("0.8.1")),
            HostVersionState::Unknown
        );
        assert_eq!(
            compare_host_versions("0.8.1", Some("unknown")),
            HostVersionState::Unknown
        );
        assert_eq!(
            compare_host_versions("0.8.1", None),
            HostVersionState::Unknown
        );
    }

    #[test]
    fn install_guard_rejects_a_newer_installed_service() {
        let error = ensure_host_install_allowed(&status(true)).unwrap_err();
        assert!(error.contains("0.9.9"));
        assert!(error.contains("0.10.0"));
        assert!(error.contains("拒绝降级"));
        assert!(ensure_host_install_allowed(&status(false)).is_ok());
    }
}
