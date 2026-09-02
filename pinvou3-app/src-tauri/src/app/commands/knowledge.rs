pub(super) fn build_kb_agentic_guide(collection_names: &[String]) -> String {
    let titles = if collection_names.is_empty() {
        "《知识集》".to_string()
    } else {
        collection_names
            .iter()
            .map(|name| format!("《{name}》"))
            .collect::<Vec<_>>()
            .join("、")
    };
    format!(
        "<system-reminder>\n\
         本会话启用了本地或远程知识集{titles}。涉及这些资料/文档的问题,你**必须先调用 \
         `kb_search` 工具**检索,再**严格基于返回的片段**作答并注明来源文件;检索不到相关\
         内容就如实告诉用户「未在知识集中找到」,**绝不凭记忆编造**。片段足够时直接回答;\
         只有需要同一来源的相邻内容时才用 `kb_open_source(source_ref=...)`,不要对 XLSX/\
         DOCX/PPTX 等来源调用 `File(action=\"read\")` 或用 `Bash(action=\"run\")` 全量展开。与本地资料无关的闲聊/常识\
         问题不必检索,正常回答即可。\n\
         </system-reminder>"
    )
}

/// 给会话挂载一个知识集(会话级粘连)。后续每条消息发送前自动检索注入。
///
/// 这是旧客户端的单知识库兼容命令，必须保持“整体替换”为一个知识集的历史语义；多知识库
/// 客户端使用下方 add/remove/enable 原子命令，不能把这里改成追加，否则旧客户端无法切换单库。
#[tauri::command]
pub async fn session_mount_collection(
    session_id: String,
    collection_id: i64,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<(), String> {
    let coordinator = knowledge.mount_mutation_coordinator();
    let _mutation = coordinator.lock().await;
    ensure_collection_mountable(&knowledge, collection_id)?;
    store.set_mounted_collection(&session_id, Some(collection_id));
    publish_kb_mount_change(
        &app,
        &session_id,
        &store.mounted_collections_snapshot(&session_id),
    );
    Ok(())
}

/// 兼容整列表替换接口。新客户端的单项操作使用下方原子命令，避免多端并发覆盖。
#[tauri::command]
pub async fn session_set_mounted_collections(
    session_id: String,
    collections: Vec<crate::features::sessions::MountedCollection>,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<Vec<crate::features::sessions::MountedCollection>, String> {
    let coordinator = knowledge.mount_mutation_coordinator();
    let _mutation = coordinator.lock().await;
    let normalized =
        validate_mount_replacement(collections, knowledge.semantic_ready(), |collection_id| {
            knowledge
                .l1()
                .collection_name(collection_id)
                .map(|name| name.is_some())
                .map_err(|error| error.to_string())
        })?;
    let snapshot = store.set_mounted_collections(&session_id, normalized);
    publish_kb_mount_change(&app, &session_id, &snapshot);
    Ok(snapshot.collections)
}

fn validate_mount_replacement<F>(
    collections: Vec<crate::features::sessions::MountedCollection>,
    semantic_ready: bool,
    mut collection_exists: F,
) -> Result<Vec<crate::features::sessions::MountedCollection>, String>
where
    F: FnMut(i64) -> Result<bool, String>,
{
    let mut normalized = Vec::new();
    for collection in collections {
        if collection.collection_id <= 0 {
            return Err("知识集 id 无效".to_string());
        }
        if normalized
            .iter()
            .any(|mounted: &crate::features::sessions::MountedCollection| {
                mounted.collection_id == collection.collection_id
            })
        {
            continue;
        }
        if !collection_exists(collection.collection_id)? {
            return Err(format!(
                "知识集 {} 不存在或已删除",
                collection.collection_id
            ));
        }
        normalized.push(collection);
    }
    if normalized.iter().any(|collection| collection.enabled) && !semantic_ready {
        return Err("embedding 模型未就绪,知识库暂不可用".to_string());
    }
    Ok(normalized)
}

fn ensure_collection_mountable(
    knowledge: &KnowledgeService,
    collection_id: i64,
) -> Result<(), String> {
    validate_collection_mountable(collection_id, knowledge.semantic_ready(), || {
        knowledge
            .l1()
            .collection_name(collection_id)
            .map(|name| name.is_some())
            .map_err(|error| error.to_string())
    })
}

fn validate_collection_mountable<F>(
    collection_id: i64,
    semantic_ready: bool,
    collection_exists: F,
) -> Result<(), String>
where
    F: FnOnce() -> Result<bool, String>,
{
    if collection_id <= 0 {
        return Err("知识集 id 无效".to_string());
    }
    if !semantic_ready {
        return Err("embedding 模型未就绪,知识库暂不可用".to_string());
    }
    if !collection_exists()? {
        return Err("知识集不存在或已删除".to_string());
    }
    Ok(())
}

/// 在 SessionStore 的同一写锁内追加或重新启用一个知识集，避免跨端 read-modify-write 丢更新。
#[tauri::command]
pub async fn session_add_mounted_collection(
    session_id: String,
    collection_id: i64,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<crate::features::sessions::MountedCollectionsSnapshot, String> {
    let coordinator = knowledge.mount_mutation_coordinator();
    let _mutation = coordinator.lock().await;
    ensure_collection_mountable(&knowledge, collection_id)?;
    let snapshot = store.add_mounted_collection(&session_id, collection_id);
    publish_kb_mount_change(&app, &session_id, &snapshot);
    Ok(snapshot)
}

/// 原子切换单个知识集的启用状态。停用不依赖模型或知识集仍然存在，以便清理陈旧状态。
#[tauri::command]
pub async fn session_set_mounted_collection_enabled(
    session_id: String,
    collection_id: i64,
    enabled: bool,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<crate::features::sessions::MountedCollectionsSnapshot, String> {
    let coordinator = knowledge.mount_mutation_coordinator();
    let _mutation = coordinator.lock().await;
    if enabled {
        ensure_collection_mountable(&knowledge, collection_id)?;
    }
    let snapshot = store.set_mounted_collection_enabled(&session_id, collection_id, enabled);
    publish_kb_mount_change(&app, &session_id, &snapshot);
    Ok(snapshot)
}

/// 原子移除单个知识集；与其他端对不同知识集的并发操作可以安全合并。
#[tauri::command]
pub async fn session_remove_mounted_collection(
    session_id: String,
    collection_id: i64,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<crate::features::sessions::MountedCollectionsSnapshot, String> {
    let coordinator = knowledge.mount_mutation_coordinator();
    let _mutation = coordinator.lock().await;
    let snapshot = store.remove_mounted_collection(&session_id, collection_id);
    publish_kb_mount_change(&app, &session_id, &snapshot);
    Ok(snapshot)
}

/// 摘下会话的知识集挂载。
#[tauri::command]
pub async fn session_unmount_collection(
    session_id: String,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<crate::features::sessions::MountedCollectionsSnapshot, String> {
    let coordinator = knowledge.mount_mutation_coordinator();
    let _mutation = coordinator.lock().await;
    let snapshot = store.set_mounted_collections(&session_id, Vec::new());
    publish_kb_mount_change(&app, &session_id, &snapshot);
    Ok(snapshot)
}

fn publish_kb_mount_change(
    app: &AppHandle,
    session_id: &str,
    snapshot: &crate::features::sessions::MountedCollectionsSnapshot,
) {
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
    });
    if let Err(error) = app.emit("remote_control:kb_mount_changed", payload.clone()) {
        // The invoking client still receives the authoritative snapshot (including revision), and
        // later events carry a full snapshot rather than a delta. Keep the mutation successful but
        // retain diagnostics for the rare case where another local window misses this notification.
        log::warn!(
            "[knowledge] failed to emit mount revision {} for session {session_id}: {error}",
            snapshot.revision
        );
    }
    crate::features::remote_control::forward_app_event(
        app,
        "remote_control:kb_mount_changed",
        payload,
    );
}

/// 读会话当前挂载的知识集 id(前端切会话时重读,恢复挂载条显示)。
#[tauri::command]
pub fn session_mounted_collection(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Option<i64> {
    store.mounted_collection(&session_id)
}

/// 读会话当前挂载的全部知识集及启用状态。
#[tauri::command]
pub fn session_mounted_collections(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Vec<crate::features::sessions::MountedCollection> {
    store.mounted_collections(&session_id)
}

/// 带修订号读取挂载事实源，供多端拒绝乱序响应。
#[tauri::command]
pub fn session_mounted_collections_snapshot(
    session_id: String,
    store: State<'_, SessionStore>,
) -> crate::features::sessions::MountedCollectionsSnapshot {
    store.mounted_collections_snapshot(&session_id)
}

use crate::features::knowledge as knowledge_domain;
use crate::features::knowledge::model_download as model_domain;
use crate::features::remote_knowledge::RemoteKnowledgeService;
use knowledge_domain::*;
use model_domain::*;

sync_command_passthrough!(knowledge_domain, kb_start_scan(state: State<'_, KnowledgeService>, roots: Option<Vec<String>>) -> ScanState);
sync_command_passthrough!(knowledge_domain, kb_scan_status(state: State<'_, KnowledgeService>) -> ScanState);
sync_command_passthrough!(knowledge_domain, kb_cancel_scan(state: State<'_, KnowledgeService>));
async_command_passthrough!(knowledge_domain, kb_type_counts(state: State<'_, KnowledgeService>) -> Result<Vec<TypeCount>, String>);
async_command_passthrough!(knowledge_domain, kb_collection_list(state: State<'_, KnowledgeService>) -> Result<Vec<Collection>, String>);
async_command_passthrough!(knowledge_domain, kb_collection_create(state: State<'_, KnowledgeService>, name: String, category: Option<String>, description: Option<String>) -> Result<i64, String>);
async_command_passthrough!(knowledge_domain, kb_collection_update(state: State<'_, KnowledgeService>, id: i64, name: String, category: Option<String>, description: Option<String>) -> Result<(), String>);

#[tauri::command]
pub async fn kb_collection_delete(
    state: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
    sessions: State<'_, SessionStore>,
    app: AppHandle,
    id: i64,
) -> Result<(), String> {
    let coordinator = state.mount_mutation_coordinator();
    let _mutation = coordinator.lock().await;
    knowledge_domain::kb_collection_delete(state, pool, id).await?;
    for (session_id, snapshot) in sessions.remove_mounted_collection_from_all(id) {
        publish_kb_mount_change(&app, &session_id, &snapshot);
    }
    Ok(())
}
sync_command_passthrough!(knowledge_domain, kb_collection_add_sources(state: State<'_, KnowledgeService>, collection_id: i64, paths: Vec<String>) -> IndexState);
sync_command_passthrough!(knowledge_domain, kb_index_status(state: State<'_, KnowledgeService>) -> IndexState);
sync_command_passthrough!(knowledge_domain, kb_index_cancel(state: State<'_, KnowledgeService>) -> Result<(), String>);
sync_command_passthrough!(knowledge_domain, kb_index_failed_files(state: State<'_, KnowledgeService>, job_id: String, offset: usize, limit: usize) -> Result<FailedImportFilePage, String>);
sync_command_passthrough!(knowledge_domain, kb_index_resume(state: State<'_, KnowledgeService>, job_id: String) -> Result<IndexState, String>);
sync_command_passthrough!(knowledge_domain, kb_index_retry_file(state: State<'_, KnowledgeService>, job_id: String, item_id: i64) -> Result<IndexState, String>);
async_command_passthrough!(knowledge_domain, kb_documents(state: State<'_, KnowledgeService>, collection_id: i64, limit: Option<usize>) -> Result<Vec<Document>, String>);
async_command_passthrough!(knowledge_domain, kb_remove_document(state: State<'_, KnowledgeService>, pool: State<'_, EnginePool>, doc_id: i64) -> Result<(), String>);
sync_command_passthrough!(knowledge_domain, kb_embed_info(state: State<'_, KnowledgeService>) -> EmbedInfo);
async_command_passthrough!(knowledge_domain, kb_search(state: State<'_, KnowledgeService>, query: SearchQueryDto) -> Result<Vec<FileHit>, String>);
async_command_passthrough!(knowledge_domain, kb_stats(state: State<'_, KnowledgeService>) -> Result<Stats, String>);

#[tauri::command]
pub async fn kb_model_status(
    app: AppHandle,
    service: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
) -> Result<KbModelStatus, String> {
    // The bundled host may finish installing the shared model after desktop
    // startup and while its owner panel is closed. A normal local status query
    // must therefore adopt the complete on-disk model instead of merely
    // reporting `installed=true, ready=false` and asking the user to retry.
    if model_domain::model_installed() && !service.semantic_ready() {
        if let Err(error) =
            model_domain::load_installed_embedder(service.inner(), pool.inner()).await
        {
            eprintln!("[knowledge] installed model hot-load during status refresh failed: {error}");
        }
    }
    let status = model_domain::kb_model_status(service);
    let _ = app.emit("kb_model:status", &status);
    Ok(status)
}
sync_command_passthrough!(model_domain, kb_model_cancel());
async_command_passthrough!(model_domain, kb_model_load_after_first_frame(app: AppHandle, service: State<'_, KnowledgeService>, pool: State<'_, EnginePool>) -> Result<bool, String>);

#[tauri::command]
pub async fn kb_model_download(
    app: AppHandle,
    service: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
    remote: State<'_, RemoteKnowledgeService>,
    repair: Option<bool>,
) -> Result<KbModelStatus, String> {
    let status = model_domain::kb_model_download(app, service, pool, repair).await?;

    // If this desktop owns the bundled loopback host, tell that process to
    // re-check the shared model directory. Its download endpoint takes the
    // cross-process install lock and follows the disk fast path, so it loads
    // this already-validated copy without downloading it a second time.
    for connection in remote
        .configured_connections()
        .into_iter()
        .filter(|item| item.scope.is_owner() && item.endpoint == "https://127.0.0.1:3210")
    {
        if let Err(error) = remote.download_model(&connection.server_id).await {
            eprintln!(
                "[knowledge] desktop model is ready, but shared host refresh failed: {error}"
            );
        }
    }

    Ok(status)
}
use super::prelude::*;

#[cfg(test)]
mod tests {
    use super::{validate_collection_mountable, validate_mount_replacement};
    use crate::features::knowledge::KnowledgeService;
    use crate::features::sessions::MountedCollection;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn replacement_rejects_deleted_collection_instead_of_silently_dropping_it() {
        let requested = vec![
            MountedCollection {
                collection_id: 7,
                enabled: true,
            },
            MountedCollection {
                collection_id: 9,
                enabled: false,
            },
        ];
        let error =
            validate_mount_replacement(requested, true, |collection_id| Ok(collection_id == 7))
                .unwrap_err();

        assert!(error.contains('9'));
    }

    #[test]
    fn replacement_deduplicates_but_keeps_the_first_requested_state() {
        let requested = vec![
            MountedCollection {
                collection_id: 7,
                enabled: false,
            },
            MountedCollection {
                collection_id: 7,
                enabled: true,
            },
        ];
        let normalized =
            validate_mount_replacement(requested, false, |_| Ok(true)).expect("disabled mount");

        assert_eq!(
            normalized,
            vec![MountedCollection {
                collection_id: 7,
                enabled: false,
            }]
        );
    }

    #[test]
    fn legacy_mount_validation_rejects_a_deleted_collection() {
        let error = validate_collection_mountable(7, true, || Ok(false)).unwrap_err();

        assert!(error.contains("不存在或已删除"));
    }

    #[tokio::test]
    async fn deletion_waits_for_validated_mount_then_removes_it() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-kb-mount-coordinator-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create test root");
        let service = KnowledgeService::new(&root.join("index.db")).expect("knowledge service");
        let exists = Arc::new(AtomicBool::new(true));
        let mounted = Arc::new(Mutex::new(Vec::new()));
        let validated = Arc::new(tokio::sync::Notify::new());

        let mount_coordinator = service.mount_mutation_coordinator();
        let delete_coordinator = service.mount_mutation_coordinator();
        let mount = {
            let exists = exists.clone();
            let mounted = mounted.clone();
            let validated = validated.clone();
            async move {
                let _mutation = mount_coordinator.lock().await;
                assert!(exists.load(Ordering::Acquire));
                validated.notify_one();
                // Give deletion a chance to contend for the same coordinator after validation.
                tokio::task::yield_now().await;
                mounted.lock().unwrap().push(7);
            }
        };
        let delete = {
            let exists = exists.clone();
            let mounted = mounted.clone();
            async move {
                validated.notified().await;
                let _mutation = delete_coordinator.lock().await;
                exists.store(false, Ordering::Release);
                mounted.lock().unwrap().retain(|id| *id != 7);
            }
        };

        tokio::join!(mount, delete);

        assert!(!exists.load(Ordering::Acquire));
        assert!(mounted.lock().unwrap().is_empty());
        drop(service);
        let _ = std::fs::remove_dir_all(root);
    }
}
