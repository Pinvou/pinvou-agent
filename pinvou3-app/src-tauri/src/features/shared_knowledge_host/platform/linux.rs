//! Linux 适配：通过安装包内的 root-owned helper 管理 systemd 服务。

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use pinvou_knowledge::client::KnowledgeClient;
use pinvou_knowledge::model::DeviceGrant;

use crate::platform::process::{output_with_timeout, output_with_timeout_and_kill_tree};

use super::super::{
    compare_host_versions, HostOwnerClaim, HostRestoreResult, HostVersionState,
    PackagedHostResources, SharedKnowledgeHostStatus, LOCAL_ENDPOINT,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_HELPER_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const INSTALL_HELPER_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const BACKUP_HELPER_TIMEOUT: Duration = Duration::from_secs(60 * 60);

pub async fn status() -> SharedKnowledgeHostStatus {
    let installed_binary = Path::new("/usr/lib/pinvou/pinvou-knowledge-server");
    let installed = installed_binary.is_file();
    let running = tokio::task::spawn_blocking(|| {
        let mut command = Command::new("systemctl");
        command.args(["is-active", "--quiet", "pinvou-knowledge.service"]);
        output_with_timeout(command, PROBE_TIMEOUT)
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    let info = if running {
        KnowledgeClient::local_health_untrusted(LOCAL_ENDPOINT)
            .await
            .ok()
    } else {
        None
    };
    let service_version = if let Some(info) = info {
        Some(info.version)
    } else if installed {
        tokio::task::spawn_blocking(installed_service_version)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let app_version = env!("CARGO_PKG_VERSION").to_string();
    let version_state = compare_host_versions(&app_version, service_version.as_deref());
    let upgrade_available = installed && version_state == HostVersionState::UpgradeAvailable;
    let client_outdated = installed && version_state == HostVersionState::ClientOutdated;
    SharedKnowledgeHostStatus {
        supported: true,
        installed,
        running,
        endpoint: LOCAL_ENDPOINT.to_string(),
        service_version,
        app_version,
        upgrade_available,
        client_outdated,
    }
}

fn installed_service_version() -> Option<String> {
    let mut command = Command::new("/usr/lib/pinvou/pinvou-knowledge-server");
    command.arg("--version");
    let output = output_with_timeout(command, PROBE_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    service_version_from_output(&output.stdout)
}

fn service_version_from_output(output: &[u8]) -> Option<String> {
    std::str::from_utf8(output)
        .ok()?
        .split_whitespace()
        .next_back()
        .map(str::to_string)
}

pub async fn install_or_upgrade(
    resources: PackagedHostResources,
    model_dir: PathBuf,
    upgrade: bool,
) -> Result<Option<HostOwnerClaim>, String> {
    tokio::task::spawn_blocking(move || {
        if !resources.helper.is_file() || !resources.server.is_file() {
            return Err("安装包缺少共享知识库服务，请重新安装 PINVOU".to_string());
        }
        let uid = command_identity("-u")?;
        let gid = command_identity("-g")?;
        let operation = if upgrade {
            "升级共享知识库"
        } else {
            "安装共享知识库"
        };
        let stdout = privileged_helper(
            &resources,
            [
                if upgrade { "upgrade" } else { "install" }.to_string(),
                resources.server.to_string_lossy().into_owned(),
                model_dir.to_string_lossy().into_owned(),
                uid,
                gid,
            ],
            operation,
            INSTALL_HELPER_TIMEOUT,
        )?;
        Ok(stdout
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<HostOwnerClaim>(line).ok()))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn command_identity(flag: &str) -> Result<String, String> {
    let mut command = Command::new("id");
    command.arg(flag);
    let output = output_with_timeout(command, PROBE_TIMEOUT)
        .map_err(|error| format!("无法识别当前 Linux 用户：{error}"))?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err("无法识别当前 Linux 用户".to_string());
    }
    Ok(value)
}

pub async fn set_owner_device(
    resources: PackagedHostResources,
    device_id: String,
    owner: bool,
) -> Result<DeviceGrant, String> {
    tokio::task::spawn_blocking(move || {
        if !resources.helper.is_file() {
            return Err("安装包缺少共享知识库管理组件，请重新安装 PINVOU".to_string());
        }
        let uid = command_identity("-u")?;
        let gid = command_identity("-g")?;
        let result = privileged_helper(
            &resources,
            [
                "set-owner".to_string(),
                device_id,
                if owner { "owner" } else { "manage" }.to_string(),
                uid,
                gid,
            ],
            "设置所有者",
            CONTROL_HELPER_TIMEOUT,
        )?;
        serde_json::from_str(&result).map_err(|_| "所有者设置结果无效".to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn consume_owner_claim(
    resources: PackagedHostResources,
) -> Result<HostOwnerClaim, String> {
    tokio::task::spawn_blocking(move || {
        let result = privileged_helper(
            &resources,
            ["claim-owner".to_string()],
            "清理本机所有者凭据",
            CONTROL_HELPER_TIMEOUT,
        )?;
        serde_json::from_str(result.trim()).map_err(|_| "本机所有者凭据无效".to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn recover_owner(resources: PackagedHostResources) -> Result<HostOwnerClaim, String> {
    tokio::task::spawn_blocking(move || {
        if !resources.helper.is_file() || !resources.server.is_file() {
            return Err("安装包缺少共享知识库管理组件，请重新安装 PINVOU".to_string());
        }
        let uid = command_identity("-u")?;
        let gid = command_identity("-g")?;
        let result = privileged_helper(
            &resources,
            [
                "recover-owner".to_string(),
                resources.server.to_string_lossy().into_owned(),
                uid,
                gid,
            ],
            "重新连接本机服务",
            CONTROL_HELPER_TIMEOUT,
        )?;
        serde_json::from_str(&result).map_err(|_| "本机所有者恢复结果无效".to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn remove_host(
    resources: PackagedHostResources,
    delete_data: bool,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        if !resources.helper.is_file() {
            return Err("安装包缺少共享知识库管理组件，请重新安装 PINVOU".to_string());
        }
        privileged_helper(
            &resources,
            [
                "remove".to_string(),
                if delete_data {
                    "delete-data"
                } else {
                    "keep-data"
                }
                .to_string(),
            ],
            "移除共享知识库",
            CONTROL_HELPER_TIMEOUT,
        )?;
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn backup_host(
    resources: PackagedHostResources,
    output: PathBuf,
    local_recipient: String,
    recovery_recipient: String,
) -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(move || {
        let uid = command_identity("-u")?;
        let gid = command_identity("-g")?;
        let result = privileged_helper(
            &resources,
            [
                "backup".to_string(),
                output.to_string_lossy().into_owned(),
                local_recipient,
                recovery_recipient,
                uid,
                gid,
            ],
            "创建共享知识库备份",
            BACKUP_HELPER_TIMEOUT,
        )?;
        result
            .lines()
            .find_map(|line| serde_json::from_str(line).ok())
            .ok_or_else(|| "备份结果无效".to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn restore_host(
    resources: PackagedHostResources,
    input: PathBuf,
    identity_file: PathBuf,
    content_only: bool,
) -> Result<HostRestoreResult, String> {
    tokio::task::spawn_blocking(move || {
        let uid = command_identity("-u")?;
        let gid = command_identity("-g")?;
        let result = privileged_helper(
            &resources,
            [
                "restore".to_string(),
                input.to_string_lossy().into_owned(),
                identity_file.to_string_lossy().into_owned(),
                if content_only {
                    "content-only"
                } else {
                    "same-host"
                }
                .to_string(),
                uid,
                gid,
            ],
            "恢复共享知识库",
            BACKUP_HELPER_TIMEOUT,
        )?;
        let mut manifest = None;
        let mut owner_claim = None;
        for line in result.lines() {
            if manifest.is_none() {
                manifest = serde_json::from_str::<serde_json::Value>(line).ok();
            }
            if owner_claim.is_none() {
                owner_claim = serde_json::from_str::<HostOwnerClaim>(line).ok();
            }
        }
        Ok(HostRestoreResult {
            manifest: manifest.ok_or_else(|| "恢复结果无效".to_string())?,
            owner_claim,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn privileged_helper<const N: usize>(
    resources: &PackagedHostResources,
    args: [String; N],
    operation: &str,
    timeout: Duration,
) -> Result<String, String> {
    if !resources.helper.is_file() {
        return Err("安装包缺少共享知识库管理组件，请重新安装 PINVOU".to_string());
    }
    let mut command = Command::new("pkexec");
    command.arg(&resources.helper).args(args);
    let output = output_with_timeout_and_kill_tree(command, timeout)
        .map_err(|error| privileged_helper_execution_error(operation, &error))?;
    if !output.status.success() {
        return Err(privileged_helper_failure(
            operation,
            output.status.code(),
            &output.stderr,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn privileged_helper_execution_error(operation: &str, error: &str) -> String {
    if error.contains(" timed out after ") {
        format!(
            "{operation}等待超时，已停止等待；系统操作可能仍在收尾，请刷新状态后再重试：{error}"
        )
    } else {
        format!("无法打开系统管理员确认：{error}")
    }
}

fn privileged_helper_failure(operation: &str, code: Option<i32>, stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr).trim().to_string();
    match code {
        // pkexec documents 126 for a dismissed authentication dialog and 127
        // when authorization could not be obtained.
        Some(126) => format!("{operation}已取消"),
        Some(127) if detail.is_empty() => format!("{operation}未获系统管理员授权"),
        Some(127) => format!("{operation}未获系统管理员授权：{detail}"),
        _ if detail.is_empty() => format!("{operation}失败"),
        _ => detail,
    }
}

pub fn lan_endpoints() -> Vec<String> {
    let mut command = Command::new("hostname");
    command.arg("-I");
    let output = output_with_timeout(command, PROBE_TIMEOUT);
    let mut endpoints = output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|value| value.parse::<IpAddr>().ok())
        .filter(|address| is_lan_address(*address))
        .map(|address| match address {
            IpAddr::V4(address) => format!("https://{address}:3210"),
            IpAddr::V6(address) => format!("https://[{address}]:3210"),
        })
        .collect::<Vec<_>>();
    endpoints.sort();
    endpoints.dedup();
    endpoints
}

fn is_lan_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private() && !address.is_loopback() && !is_tailnet(address)
        }
        // Only ULA addresses are portable between machines. A link-local fe80::
        // endpoint requires an interface zone id, which `hostname -I` does not
        // provide and therefore must never be advertised in a share link.
        IpAddr::V6(address) => !address.is_loopback() && (address.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn is_tailnet(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_endpoints_exclude_loopback_public_and_tailnet_addresses() {
        for address in ["192.168.1.20", "10.20.0.3", "fd12::3"] {
            assert!(is_lan_address(address.parse().unwrap()), "{address}");
        }
        for address in ["127.0.0.1", "8.8.8.8", "100.64.12.34", "::1", "fe80::1"] {
            assert!(!is_lan_address(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn installed_binary_version_is_available_while_the_service_is_stopped() {
        assert_eq!(
            service_version_from_output(b"pinvou-knowledge-server 0.10.0\n"),
            Some("0.10.0".to_string())
        );
        assert_eq!(service_version_from_output(b""), None);
        assert_eq!(service_version_from_output(&[0xff]), None);
    }

    #[test]
    fn pkexec_cancellation_authorization_and_timeout_are_distinct() {
        assert_eq!(
            privileged_helper_failure("恢复共享知识库", Some(126), b"dismissed"),
            "恢复共享知识库已取消"
        );
        assert_eq!(
            privileged_helper_failure("恢复共享知识库", Some(127), b""),
            "恢复共享知识库未获系统管理员授权"
        );
        assert_eq!(
            privileged_helper_failure("恢复共享知识库", Some(1), b"restore failed"),
            "restore failed"
        );
        assert!(privileged_helper_execution_error(
            "恢复共享知识库",
            "pkexec timed out after 3600s: no subprocess output"
        )
        .contains("等待超时"));
    }
}
