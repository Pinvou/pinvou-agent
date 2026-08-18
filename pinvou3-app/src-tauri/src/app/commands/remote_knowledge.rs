use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use pinvou_knowledge::client::{KnowledgeClient, RemoteKnowledgeProbe};
use pinvou_knowledge::model::{
    AccessScope, Collection, DeviceGrant, Document, JoinRequestRecord, ModelStatus, SearchHit,
    ShareCreated, ShareRecord, TrashedDocument, UpdateDeviceRequest,
};
use tauri::{AppHandle, Emitter, State};

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::knowledge::{model_download as local_model, KnowledgeService};
use crate::features::remote_knowledge::{
    discover_folder_files, JoinOutcome, PendingJoin, RemoteConnectionStatus, RemoteFolderDiscovery,
    RemoteKnowledgeIdentity, RemoteKnowledgeService,
};
use crate::features::sessions::{MountedRemoteCollection, SessionStore};

type RemoteMountMutationMap = HashMap<(String, i64), Weak<tokio::sync::Mutex<()>>>;
static REMOTE_MOUNT_MUTATIONS: OnceLock<Mutex<RemoteMountMutationMap>> = OnceLock::new();
type RemoteServerMutationMap = HashMap<String, Weak<Mutex<()>>>;
static REMOTE_SERVER_MUTATIONS: OnceLock<Mutex<RemoteServerMutationMap>> = OnceLock::new();

fn remote_mount_mutation_coordinator(
    server_id: &str,
    collection_id: i64,
) -> Arc<tokio::sync::Mutex<()>> {
    let coordinators = REMOTE_MOUNT_MUTATIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut coordinators = coordinators
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    coordinators.retain(|_, coordinator| coordinator.strong_count() > 0);
    let key = (server_id.to_string(), collection_id);
    if let Some(coordinator) = coordinators.get(&key).and_then(Weak::upgrade) {
        return coordinator;
    }
    let coordinator = Arc::new(tokio::sync::Mutex::new(()));
    coordinators.insert(key, Arc::downgrade(&coordinator));
    coordinator
}

fn remote_server_mutation_coordinator(server_id: &str) -> Arc<Mutex<()>> {
    let coordinators = REMOTE_SERVER_MUTATIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut coordinators = coordinators
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    coordinators.retain(|_, coordinator| coordinator.strong_count() > 0);
    if let Some(coordinator) = coordinators.get(server_id).and_then(Weak::upgrade) {
        return coordinator;
    }
    let coordinator = Arc::new(Mutex::new(()));
    coordinators.insert(server_id.to_string(), Arc::downgrade(&coordinator));
    coordinator
}

#[tauri::command]
pub async fn remote_kb_connections(
    state: State<'_, RemoteKnowledgeService>,
) -> Result<Vec<RemoteConnectionStatus>, String> {
    state.statuses().await
}

#[tauri::command]
pub async fn remote_kb_request_join(
    state: State<'_, RemoteKnowledgeService>,
    source: String,
    device_name: String,
) -> Result<JoinOutcome, String> {
    state.request_join(&source, &device_name).await
}

#[tauri::command]
pub async fn remote_kb_probe_private_endpoint(
    source: String,
) -> Result<RemoteKnowledgeProbe, String> {
    KnowledgeClient::probe_private_identity(&source).await
}

#[tauri::command]
pub async fn remote_kb_request_join_confirmed(
    state: State<'_, RemoteKnowledgeService>,
    probe: RemoteKnowledgeProbe,
    device_name: String,
    confirmed_ca_fingerprint: String,
    confirmed_identity_code: String,
) -> Result<JoinOutcome, String> {
    state
        .request_join_confirmed(
            probe,
            &device_name,
            &confirmed_ca_fingerprint,
            &confirmed_identity_code,
        )
        .await
}

#[tauri::command]
pub fn remote_kb_connection_identity(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
) -> Result<RemoteKnowledgeIdentity, String> {
    state.connection_identity(&server_id)
}

#[tauri::command]
pub fn remote_kb_pending_joins(state: State<'_, RemoteKnowledgeService>) -> Vec<PendingJoin> {
    state.pending_joins()
}

#[tauri::command]
pub async fn remote_kb_refresh_join(
    state: State<'_, RemoteKnowledgeService>,
    request_id: String,
) -> Result<JoinOutcome, String> {
    state.refresh_join(&request_id).await
}

#[tauri::command]
pub async fn remote_kb_cancel_join(
    state: State<'_, RemoteKnowledgeService>,
    request_id: String,
) -> Result<JoinRequestRecord, String> {
    state.cancel_join(&request_id).await
}

#[tauri::command]
pub async fn remote_kb_create_share(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    endpoints: Vec<String>,
    auto_approve_read: bool,
) -> Result<ShareCreated, String> {
    state
        .create_share(&server_id, endpoints, auto_approve_read)
        .await
}

#[tauri::command]
pub async fn remote_kb_shares(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
) -> Result<Vec<ShareRecord>, String> {
    state.shares(&server_id).await
}

#[tauri::command]
pub async fn remote_kb_stop_share(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    share_id: String,
) -> Result<ShareRecord, String> {
    state.stop_share(&server_id, &share_id).await
}

#[tauri::command]
pub async fn remote_kb_join_requests(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
) -> Result<Vec<JoinRequestRecord>, String> {
    state.join_requests(&server_id).await
}

#[tauri::command]
pub async fn remote_kb_approve_join_request(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    request_id: String,
    scope: AccessScope,
) -> Result<JoinRequestRecord, String> {
    state
        .approve_join_request(&server_id, &request_id, scope)
        .await
}

#[tauri::command]
pub async fn remote_kb_reject_join_request(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    request_id: String,
) -> Result<JoinRequestRecord, String> {
    state.reject_join_request(&server_id, &request_id).await
}

#[tauri::command]
pub async fn remote_kb_model_status(
    app: AppHandle,
    state: State<'_, RemoteKnowledgeService>,
    local_knowledge: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
    server_id: String,
) -> Result<ModelStatus, String> {
    let remote_status = state.model_status(&server_id).await?;
    if !remote_status.downloading {
        sync_peer_installed_local_model(&app, local_knowledge.inner(), pool.inner()).await;
    }
    Ok(remote_status)
}

#[tauri::command]
pub async fn remote_kb_download_model(
    app: AppHandle,
    state: State<'_, RemoteKnowledgeService>,
    local_knowledge: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
    server_id: String,
) -> Result<ModelStatus, String> {
    let remote_status = state.download_model(&server_id).await?;

    // The bundled Linux host and the desktop process intentionally share the
    // same user-owned model directory. A host download can therefore make the
    // local model appear after desktop startup. Load that peer-installed copy
    // in-process and publish an authoritative status so the startup snapshot
    // cannot keep the local UI stuck on "not installed".
    if !remote_status.downloading {
        sync_peer_installed_local_model(&app, local_knowledge.inner(), pool.inner()).await;
    }

    Ok(remote_status)
}

async fn sync_peer_installed_local_model(
    app: &AppHandle,
    local_knowledge: &KnowledgeService,
    pool: &EnginePool,
) {
    if !local_model::model_installed() {
        return;
    }
    if let Err(error) = local_model::load_installed_embedder(local_knowledge, pool).await {
        eprintln!(
            "[knowledge] shared host model is installed, but desktop hot-load failed: {error}"
        );
    }
    let local_status = local_model::current_status(local_knowledge);
    let _ = app.emit("kb_model:status", &local_status);
}

#[tauri::command]
pub async fn remote_kb_devices(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
) -> Result<Vec<DeviceGrant>, String> {
    state.devices(&server_id).await
}

#[tauri::command]
pub async fn remote_kb_update_device(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    device_id: String,
    name: Option<String>,
    scope: Option<AccessScope>,
    revoked: Option<bool>,
) -> Result<DeviceGrant, String> {
    state
        .update_device(
            &server_id,
            &device_id,
            UpdateDeviceRequest {
                name,
                scope,
                revoked,
            },
        )
        .await
}

#[tauri::command]
pub async fn remote_kb_remove_device(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    device_id: String,
) -> Result<(), String> {
    state.remove_device(&server_id, &device_id).await
}

#[tauri::command]
pub async fn remote_kb_trashed_collections(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
) -> Result<Vec<Collection>, String> {
    state.trashed_collections(&server_id).await
}

#[tauri::command]
pub async fn remote_kb_trashed_documents(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
) -> Result<Vec<TrashedDocument>, String> {
    state.trashed_documents(&server_id).await
}

#[tauri::command]
pub async fn remote_kb_permanently_delete_collection(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    id: i64,
) -> Result<(), String> {
    state.permanently_delete_collection(&server_id, id).await
}

#[tauri::command]
pub async fn remote_kb_permanently_delete_document(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    id: i64,
) -> Result<(), String> {
    state.permanently_delete_document(&server_id, id).await
}

#[tauri::command]
pub fn remote_kb_remove_connection(
    state: State<'_, RemoteKnowledgeService>,
    sessions: State<'_, SessionStore>,
    app: AppHandle,
    server_id: String,
) -> Result<(), String> {
    remove_connection_with_mounts(state.inner(), sessions.inner(), &app, &server_id)
}

pub(crate) fn remove_connection_with_mounts(
    state: &RemoteKnowledgeService,
    sessions: &SessionStore,
    app: &AppHandle,
    server_id: &str,
) -> Result<(), String> {
    let changed = {
        // Only the final local commit is serialized with mounting. Network validation happens
        // outside this lock, so disconnecting an offline server remains immediate.
        let coordinator = remote_server_mutation_coordinator(&server_id);
        let _mutation = coordinator
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove_connection(server_id)?;
        sessions.remove_remote_server_mounts(server_id)
    };
    for (session_id, remote_collections) in changed {
        publish_remote_kb_mount_change(app, sessions, &session_id, &remote_collections);
    }
    Ok(())
}

#[tauri::command]
pub async fn remote_kb_collections(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    include_deleted: Option<bool>,
) -> Result<Vec<Collection>, String> {
    state
        .collections(&server_id, include_deleted.unwrap_or(false))
        .await
}

#[tauri::command]
pub async fn remote_kb_create_collection(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    name: String,
    description: Option<String>,
) -> Result<Collection, String> {
    state.create_collection(&server_id, name, description).await
}

#[tauri::command]
pub async fn remote_kb_update_collection(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    id: i64,
    name: String,
    description: Option<String>,
) -> Result<Collection, String> {
    state
        .update_collection(&server_id, id, name, description)
        .await
}

#[tauri::command]
pub async fn remote_kb_delete_collection(
    state: State<'_, RemoteKnowledgeService>,
    sessions: State<'_, SessionStore>,
    app: AppHandle,
    server_id: String,
    id: i64,
) -> Result<(), String> {
    let coordinator = remote_mount_mutation_coordinator(&server_id, id);
    let _mutation = coordinator.lock().await;
    state.delete_collection(&server_id, id).await?;
    for (session_id, remote_collections) in
        sessions.remove_mounted_remote_collection_from_all(&server_id, id)
    {
        publish_remote_kb_mount_change(&app, sessions.inner(), &session_id, &remote_collections);
    }
    Ok(())
}

fn publish_remote_kb_mount_change(
    app: &AppHandle,
    sessions: &SessionStore,
    session_id: &str,
    remote_collections: &[MountedRemoteCollection],
) {
    // Preserve the existing local-mount event contract while extending it with the authoritative
    // remote list. Older clients re-read the local snapshot; newer clients can update remote chips
    // immediately without another IPC round trip.
    let snapshot = sessions.mounted_collections_snapshot(session_id);
    let collection_id = snapshot
        .collections
        .iter()
        .find(|collection| collection.enabled)
        .map(|collection| collection.collection_id);
    let payload = serde_json::json!({
        "session_id": session_id,
        "collection_id": collection_id,
        "collections": &snapshot.collections,
        "revision": snapshot.revision,
        "remote_collections": remote_collections,
    });
    if let Err(error) = app.emit("remote_control:kb_mount_changed", payload.clone()) {
        log::warn!(
            "[remote-knowledge] failed to emit mount change for session {session_id}: {error}"
        );
    }
    crate::features::remote_control::forward_app_event(
        app,
        "remote_control:kb_mount_changed",
        payload,
    );
}

#[tauri::command]
pub async fn remote_kb_restore_collection(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    id: i64,
) -> Result<(), String> {
    state.restore_collection(&server_id, id).await
}

#[tauri::command]
pub async fn remote_kb_documents(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    collection_id: i64,
    include_deleted: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<Document>, String> {
    let include_deleted = include_deleted.unwrap_or(false);
    match (limit, offset) {
        (None, None) => {
            state
                .documents(&server_id, collection_id, include_deleted)
                .await
        }
        (limit, offset) => {
            state
                .documents_page(
                    &server_id,
                    collection_id,
                    include_deleted,
                    limit,
                    offset.unwrap_or(0),
                )
                .await
        }
    }
}

#[tauri::command]
pub async fn remote_kb_document_statuses(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    document_ids: Vec<i64>,
) -> Result<Vec<Document>, String> {
    state.document_statuses(&server_id, &document_ids).await
}

#[tauri::command]
pub async fn remote_kb_discover_folder_files(
    paths: Vec<String>,
) -> Result<RemoteFolderDiscovery, String> {
    tokio::task::spawn_blocking(move || discover_folder_files(&paths))
        .await
        .map_err(|error| format!("扫描文件夹任务失败：{error}"))?
}

#[tauri::command]
pub async fn remote_kb_upload_files(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    collection_id: i64,
    paths: Vec<String>,
) -> Result<Vec<Document>, String> {
    state.upload_paths(&server_id, collection_id, paths).await
}

#[tauri::command]
pub async fn remote_kb_replace_document(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    document_id: i64,
    path: String,
) -> Result<Document, String> {
    state.replace_document(&server_id, document_id, path).await
}

#[tauri::command]
pub async fn remote_kb_delete_document(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    id: i64,
) -> Result<(), String> {
    state.delete_document(&server_id, id).await
}

#[tauri::command]
pub async fn remote_kb_restore_document(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    id: i64,
) -> Result<(), String> {
    state.restore_document(&server_id, id).await
}

#[tauri::command]
pub async fn remote_kb_download_document(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    id: i64,
    destination: String,
) -> Result<String, String> {
    state.download_document(&server_id, id, destination).await
}

#[tauri::command]
pub async fn remote_kb_search(
    state: State<'_, RemoteKnowledgeService>,
    server_id: String,
    collection_ids: Vec<i64>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<SearchHit>, String> {
    state
        .search(
            &server_id,
            collection_ids,
            query,
            limit.unwrap_or(8).clamp(1, 50),
        )
        .await
}

#[tauri::command]
pub fn session_mounted_remote_collections(
    store: State<'_, SessionStore>,
    session_id: String,
) -> Vec<MountedRemoteCollection> {
    store.mounted_remote_collections(&session_id)
}

#[tauri::command]
pub async fn session_add_mounted_remote_collection(
    remote: State<'_, RemoteKnowledgeService>,
    store: State<'_, SessionStore>,
    session_id: String,
    server_id: String,
    collection_id: i64,
) -> Result<Vec<MountedRemoteCollection>, String> {
    let coordinator = remote_mount_mutation_coordinator(&server_id, collection_id);
    let _mutation = coordinator.lock().await;
    ensure_remote_collection_mountable(&remote, &server_id, collection_id).await?;
    let server_coordinator = remote_server_mutation_coordinator(&server_id);
    let _server_mutation = server_coordinator
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // The connection may have been removed while the network validation was in flight.
    remote.client_for(&server_id)?;
    Ok(store.add_mounted_remote_collection(&session_id, server_id, collection_id))
}

#[tauri::command]
pub async fn session_set_mounted_remote_collection_enabled(
    remote: State<'_, RemoteKnowledgeService>,
    store: State<'_, SessionStore>,
    session_id: String,
    server_id: String,
    collection_id: i64,
    enabled: bool,
) -> Result<Vec<MountedRemoteCollection>, String> {
    let coordinator = remote_mount_mutation_coordinator(&server_id, collection_id);
    let _mutation = coordinator.lock().await;
    if enabled {
        ensure_remote_collection_mountable(&remote, &server_id, collection_id).await?;
        let server_coordinator = remote_server_mutation_coordinator(&server_id);
        let _server_mutation = server_coordinator
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        remote.client_for(&server_id)?;
        return Ok(store.set_mounted_remote_collection_enabled(
            &session_id,
            &server_id,
            collection_id,
            true,
        ));
    }
    Ok(store.set_mounted_remote_collection_enabled(&session_id, &server_id, collection_id, false))
}

#[tauri::command]
pub fn session_remove_mounted_remote_collection(
    store: State<'_, SessionStore>,
    session_id: String,
    server_id: String,
    collection_id: i64,
) -> Vec<MountedRemoteCollection> {
    store.remove_mounted_remote_collection(&session_id, &server_id, collection_id)
}

async fn ensure_remote_collection_mountable(
    remote: &RemoteKnowledgeService,
    server_id: &str,
    collection_id: i64,
) -> Result<(), String> {
    if collection_id <= 0 {
        return Err("远程知识集 ID 无效".to_string());
    }
    let info = remote.client_for(server_id)?.health().await?;
    if !info.ready {
        return Err("远程知识库模型尚未就绪，请联系服务器管理员".to_string());
    }
    let exists = remote
        .collections(server_id, false)
        .await?
        .into_iter()
        .any(|collection| collection.id == collection_id);
    if !exists {
        return Err("远程知识集不存在或已移入回收站".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{remote_mount_mutation_coordinator, remote_server_mutation_coordinator};

    #[test]
    fn remote_mount_mutations_share_only_an_exact_collection_coordinator() {
        let exact = remote_mount_mutation_coordinator("cube", 7);
        let same = remote_mount_mutation_coordinator("cube", 7);
        let other_collection = remote_mount_mutation_coordinator("cube", 8);
        let other_server = remote_mount_mutation_coordinator("other", 7);

        assert!(std::sync::Arc::ptr_eq(&exact, &same));
        assert!(!std::sync::Arc::ptr_eq(&exact, &other_collection));
        assert!(!std::sync::Arc::ptr_eq(&exact, &other_server));
    }

    #[test]
    fn remote_server_mutations_share_only_the_same_server_coordinator() {
        let exact = remote_server_mutation_coordinator("cube");
        let same = remote_server_mutation_coordinator("cube");
        let other = remote_server_mutation_coordinator("other");

        assert!(std::sync::Arc::ptr_eq(&exact, &same));
        assert!(!std::sync::Arc::ptr_eq(&exact, &other));
    }
}
