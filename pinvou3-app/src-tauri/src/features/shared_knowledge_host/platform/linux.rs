//! Linux 适配：通过安装包内的 root-owned helper 管理 systemd 服务。

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Command;

use pinvou_knowledge::client::KnowledgeClient;
use pinvou_knowledge::model::DeviceGrant;

use super::super::{
    HostOwnerClaim, HostRestoreResult, PackagedHostResources, SharedKnowledgeHostStatus,
    LOCAL_ENDPOINT,
};

pub async fn status() -> SharedKnowledgeHostStatus {
    let installed = Path::new("/usr/lib/pinvou/pinvou-knowledge-server").is_file();
    let running = tokio::task::spawn_blocking(|| {
        Command::new("systemctl")
            .args(["is-active", "--quiet", "pinvou-knowledge.service"])
            .status()
            .map(|status| status.success())
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
    let service_version = info.map(|value| value.version);
    let app_version = env!("CARGO_PKG_VERSION").to_string();
    let upgrade_available = installed && service_version.as_deref() != Some(app_version.as_str());
    SharedKnowledgeHostStatus {
        supported: true,
        installed,
        running,
        endpoint: LOCAL_ENDPOINT.to_string(),
        service_version,
        app_version,
        upgrade_available,
    }
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
        let output = Command::new("pkexec")
            .arg(&resources.helper)
            .arg(if upgrade { "upgrade" } else { "install" })
            .arg(&resources.server)
            .arg(model_dir)
            .arg(uid)
            .arg(gid)
            .output()
            .map_err(|error| format!("无法打开系统管理员确认：{error}"))?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if message.is_empty() {
                "共享知识库安装已取消或失败".to_string()
            } else {
                message
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<HostOwnerClaim>(line).ok()))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn command_identity(flag: &str) -> Result<String, String> {
    let output = Command::new("id")
        .arg(flag)
        .output()
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
        let output = Command::new("pkexec")
            .arg(&resources.helper)
            .arg("set-owner")
            .arg(device_id)
            .arg(if owner { "owner" } else { "manage" })
            .arg(uid)
            .arg(gid)
            .output()
            .map_err(|error| format!("无法打开系统管理员确认：{error}"))?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if message.is_empty() {
                "所有者设置已取消或失败".to_string()
            } else {
                message
            });
        }
        serde_json::from_slice(&output.stdout).map_err(|_| "所有者设置结果无效".to_string())
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
        let output = Command::new("pkexec")
            .arg(&resources.helper)
            .arg("recover-owner")
            .arg(&resources.server)
            .arg(uid)
            .arg(gid)
            .output()
            .map_err(|error| format!("无法打开系统管理员确认：{error}"))?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if message.is_empty() {
                "重新连接本机服务已取消或失败".to_string()
            } else {
                message
            });
        }
        serde_json::from_slice(&output.stdout).map_err(|_| "本机所有者恢复结果无效".to_string())
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
        let output = Command::new("pkexec")
            .arg(&resources.helper)
            .arg("remove")
            .arg(if delete_data {
                "delete-data"
            } else {
                "keep-data"
            })
            .output()
            .map_err(|error| format!("无法打开系统管理员确认：{error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if message.is_empty() {
                "移除共享知识库已取消或失败".to_string()
            } else {
                message
            })
        }
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
) -> Result<String, String> {
    if !resources.helper.is_file() {
        return Err("安装包缺少共享知识库管理组件，请重新安装 PINVOU".to_string());
    }
    let output = Command::new("pkexec")
        .arg(&resources.helper)
        .args(args)
        .output()
        .map_err(|error| format!("无法打开系统管理员确认：{error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!("{operation}已取消或失败")
        } else {
            message
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn lan_endpoints() -> Vec<String> {
    let output = Command::new("hostname").arg("-I").output();
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
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            !address.is_loopback() && ((first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80)
        }
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
        for address in ["127.0.0.1", "8.8.8.8", "100.64.12.34", "::1"] {
            assert!(!is_lan_address(address.parse().unwrap()), "{address}");
        }
    }
}
