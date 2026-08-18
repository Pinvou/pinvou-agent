use std::{io::Write, path::PathBuf, str::FromStr, time::Duration};

use age::secrecy::ExposeSecret;
use futures_util::future::join_all;
use pinvou_knowledge::client::{KnowledgeClient, RemoteKnowledgeProbe};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::features::knowledge;
use crate::features::remote_knowledge::{RemoteConnection, RemoteKnowledgeService};
use crate::features::sessions::SessionStore;
use crate::features::shared_knowledge_host::{self, packaged_resources, SharedKnowledgeHostStatus};

const HOST_PROGRESS_EVENT: &str = "shared-knowledge-host-progress";
static HOST_LIFECYCLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostProgress {
    operation: &'static str,
    phase: &'static str,
    percent: u8,
    error: Option<String>,
}

fn emit_host_progress(
    app: &AppHandle,
    operation: &'static str,
    phase: &'static str,
    percent: u8,
    error: Option<String>,
) {
    let _ = app.emit(
        HOST_PROGRESS_EVENT,
        HostProgress {
            operation,
            phase,
            percent,
            error,
        },
    );
}

#[tauri::command]
pub async fn shared_kb_host_status() -> SharedKnowledgeHostStatus {
    shared_knowledge_host::status().await
}

#[tauri::command]
pub fn shared_kb_host_lan_endpoints() -> Vec<String> {
    shared_knowledge_host::lan_endpoints()
}

#[tauri::command]
pub async fn shared_kb_discover_nearby() -> Result<Vec<RemoteKnowledgeProbe>, String> {
    let candidates = tokio::task::spawn_blocking(|| {
        pinvou_knowledge::discovery::discover_lan_candidates(Duration::from_millis(1500))
    })
    .await
    .map_err(|error| format!("局域网发现任务失败：{error}"))??;
    // Bound active probes even if a noisy or hostile LAN floods mDNS results.
    let probes = join_all(candidates.into_iter().take(32).map(|candidate| async move {
        KnowledgeClient::probe_private_identity(&candidate.endpoint).await
    }))
    .await;
    let mut discovered = probes
        .into_iter()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    discovered.sort_by(|left, right| {
        left.server_name
            .cmp(&right.server_name)
            .then_with(|| left.endpoint.cmp(&right.endpoint))
    });
    Ok(discovered)
}

#[tauri::command]
pub async fn shared_kb_host_install(
    app: AppHandle,
    remote: State<'_, RemoteKnowledgeService>,
) -> Result<RemoteConnection, String> {
    install_or_upgrade(&app, remote.inner(), false).await
}

#[tauri::command]
pub async fn shared_kb_host_upgrade(
    app: AppHandle,
    remote: State<'_, RemoteKnowledgeService>,
) -> Result<SharedKnowledgeHostStatus, String> {
    let _ = install_or_upgrade(&app, remote.inner(), true).await?;
    Ok(shared_knowledge_host::status().await)
}

#[tauri::command]
pub async fn shared_kb_host_reconnect(
    app: AppHandle,
    remote: State<'_, RemoteKnowledgeService>,
) -> Result<RemoteConnection, String> {
    let _host_guard = HOST_LIFECYCLE_LOCK.lock().await;
    emit_host_progress(&app, "reconnect", "prepare", 10, None);
    let result = reconnect_host_inner(&app, remote.inner()).await;
    if let Err(error) = &result {
        emit_host_progress(&app, "reconnect", "failed", 100, Some(error.clone()));
    }
    result
}

async fn reconnect_host_inner(
    app: &AppHandle,
    remote: &RemoteKnowledgeService,
) -> Result<RemoteConnection, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    emit_host_progress(app, "reconnect", "install", 45, None);
    let claim = shared_knowledge_host::recover_owner(packaged_resources(&resource_dir)).await?;
    emit_host_progress(app, "reconnect", "connect", 82, None);
    let connection = remote
        .register_local_owner("127.0.0.1:3210", &claim.token)
        .await?;
    if connection.server_id != claim.server_id || connection.device_id != claim.device_id {
        return Err("本机所有者凭据与服务身份不一致".to_string());
    }
    consume_matching_owner_claim(app, &claim).await?;
    emit_host_progress(app, "reconnect", "complete", 100, None);
    Ok(connection)
}

#[tauri::command]
pub async fn shared_kb_host_set_owner_device(
    app: AppHandle,
    device_id: String,
    owner: bool,
) -> Result<pinvou_knowledge::model::DeviceGrant, String> {
    let _host_guard = HOST_LIFECYCLE_LOCK.lock().await;
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    shared_knowledge_host::set_owner_device(packaged_resources(&resource_dir), device_id, owner)
        .await
}

#[tauri::command]
pub async fn shared_kb_host_remove(
    app: AppHandle,
    remote: State<'_, RemoteKnowledgeService>,
    sessions: State<'_, SessionStore>,
    server_id: String,
    delete_data: bool,
) -> Result<(), String> {
    let _host_guard = HOST_LIFECYCLE_LOCK.lock().await;
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    shared_knowledge_host::remove_host(packaged_resources(&resource_dir), delete_data).await?;
    if delete_data {
        crate::app::commands::remote_knowledge::remove_connection_with_mounts(
            remote.inner(),
            sessions.inner(),
            &app,
            &server_id,
        )?;
    }
    Ok(())
}

#[tauri::command]
pub async fn shared_kb_host_backup(
    app: AppHandle,
    remote: State<'_, RemoteKnowledgeService>,
    destination: PathBuf,
) -> Result<shared_knowledge_host::HostBackupResult, String> {
    let _host_guard = HOST_LIFECYCLE_LOCK.lock().await;
    let local_identity = match remote.shared_backup_identity()? {
        Some(identity) => age::x25519::Identity::from_str(&identity)
            .map_err(|_| "本机备份密钥已损坏，请修复系统凭据库".to_string())?,
        None => {
            let identity = age::x25519::Identity::generate();
            remote.set_shared_backup_identity(identity.to_string().expose_secret())?;
            identity
        }
    };
    let recovery_identity = age::x25519::Identity::generate();
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    let manifest = shared_knowledge_host::backup_host(
        packaged_resources(&resource_dir),
        destination,
        local_identity.to_public().to_string(),
        recovery_identity.to_public().to_string(),
    )
    .await?;
    Ok(shared_knowledge_host::HostBackupResult {
        manifest,
        recovery_code: recovery_identity.to_string().expose_secret().to_string(),
    })
}

#[tauri::command]
pub async fn shared_kb_host_restore(
    app: AppHandle,
    remote: State<'_, RemoteKnowledgeService>,
    sessions: State<'_, SessionStore>,
    server_id: String,
    source: PathBuf,
    recovery_code: Option<String>,
) -> Result<RemoteConnection, String> {
    let _host_guard = HOST_LIFECYCLE_LOCK.lock().await;
    let content_only = recovery_code
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    let identity = if content_only {
        recovery_code.unwrap_or_default()
    } else {
        remote
            .shared_backup_identity()?
            .ok_or_else(|| "本机备份密钥不存在，请使用该备份的恢复码".to_string())?
    };
    age::x25519::Identity::from_str(identity.trim())
        .map_err(|_| "恢复码或本机备份密钥无效".to_string())?;
    let temporary_dir = crate::platform::paths::pinvou3_home().join("knowledge");
    std::fs::create_dir_all(&temporary_dir)
        .map_err(|error| format!("无法准备恢复密钥：{error}"))?;
    let mut identity_file = tempfile::NamedTempFile::new_in(temporary_dir)
        .map_err(|error| format!("无法准备恢复密钥：{error}"))?;
    identity_file
        .write_all(identity.trim().as_bytes())
        .and_then(|_| identity_file.as_file().sync_all())
        .map_err(|error| format!("无法准备恢复密钥：{error}"))?;

    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    let restored = shared_knowledge_host::restore_host(
        packaged_resources(&resource_dir),
        source,
        identity_file.path().to_path_buf(),
        content_only,
    )
    .await?;
    let _ = restored.manifest;
    if content_only {
        let claim = restored
            .owner_claim
            .ok_or_else(|| "迁移已完成，但没有取得新主机所有者凭据".to_string())?;
        if claim.server_id == server_id {
            return Err("迁移恢复没有生成新的主机身份，已保留所有者凭据以便重试".to_string());
        }
        // 迁移恢复已替换服务身份，指向旧 server_id 的连接与挂载随即失效；先清理
        // 旧连接再注册新连接，避免注册成功后才清理失败、残留指向已失效身份的连接。
        // 若此后注册失败，所有者凭据尚未消费，可按原流程重试。
        crate::app::commands::remote_knowledge::remove_connection_with_mounts(
            remote.inner(),
            sessions.inner(),
            &app,
            &server_id,
        )?;
        let connection = remote
            .register_local_owner("127.0.0.1:3210", &claim.token)
            .await?;
        consume_matching_owner_claim(&app, &claim).await?;
        return Ok(connection);
    }
    remote
        .configured_connections()
        .into_iter()
        .find(|connection| connection.server_id == server_id && connection.scope.is_owner())
        .ok_or_else(|| "本机恢复完成，但所有者连接不存在".to_string())
}

async fn install_or_upgrade(
    app: &AppHandle,
    remote: &RemoteKnowledgeService,
    upgrade: bool,
) -> Result<RemoteConnection, String> {
    let _host_guard = HOST_LIFECYCLE_LOCK.lock().await;
    let operation = if upgrade { "upgrade" } else { "install" };
    emit_host_progress(app, operation, "prepare", 10, None);
    let result = install_or_upgrade_inner(app, remote, upgrade, operation).await;
    if let Err(error) = &result {
        emit_host_progress(app, operation, "failed", 100, Some(error.clone()));
    }
    result
}

async fn install_or_upgrade_inner(
    app: &AppHandle,
    remote: &RemoteKnowledgeService,
    upgrade: bool,
    operation: &'static str,
) -> Result<RemoteConnection, String> {
    let status = shared_knowledge_host::status().await;
    shared_knowledge_host::ensure_host_install_allowed(&status)?;
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    emit_host_progress(app, operation, "install", 45, None);
    let claim = shared_knowledge_host::install_or_upgrade(
        packaged_resources(&resource_dir),
        knowledge::model_dir(),
        upgrade,
    )
    .await?;
    emit_host_progress(app, operation, "connect", 82, None);
    let connection = if let Some(claim) = claim {
        let connection = remote
            .register_local_owner("127.0.0.1:3210", &claim.token)
            .await?;
        if connection.server_id != claim.server_id || connection.device_id != claim.device_id {
            return Err("本机所有者凭据与服务身份不一致".to_string());
        }
        consume_matching_owner_claim(app, &claim).await?;
        connection
    } else {
        remote.rebind_local_owner("127.0.0.1:3210").await?
    };
    emit_host_progress(app, operation, "complete", 100, None);
    Ok(connection)
}

async fn consume_matching_owner_claim(
    app: &AppHandle,
    expected: &shared_knowledge_host::HostOwnerClaim,
) -> Result<(), String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    let consumed =
        shared_knowledge_host::consume_owner_claim(packaged_resources(&resource_dir)).await?;
    if consumed.server_id != expected.server_id
        || consumed.device_id != expected.device_id
        || consumed.token != expected.token
    {
        return Err("本机所有者凭据在保存期间发生变化，请重新打开共享知识库".to_string());
    }
    Ok(())
}
