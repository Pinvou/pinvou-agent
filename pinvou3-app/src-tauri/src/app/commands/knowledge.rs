pub(super) fn build_kb_agentic_guide(collection_names: &[String]) -> String {
    let titles = if collection_names.is_empty() {
        "《本地知识集》".to_string()
    } else {
        collection_names
            .iter()
            .map(|name| format!("《{name}》"))
            .collect::<Vec<_>>()
            .join("、")
    };
    format!(
        "<system-reminder>\n\
         本会话启用了知识集{titles}。涉及用户本地资料/文档的问题,你**必须先调用 \
         `kb_search` 工具**检索,再**严格基于返回的片段**作答并注明来源文件;检索不到相关\
         内容就如实告诉用户「未在知识集中找到」,**绝不凭记忆编造**。片段足够时直接回答;\
         只有需要同一来源的相邻内容时才用 `kb_open_source(source_ref=...)`,不要对 XLSX/\
         DOCX/PPTX 等来源调用 `read_file` 或用 shell 全量展开。与本地资料无关的闲聊/常识\
         问题不必检索,正常回答即可。\n\
         </system-reminder>"
    )
}

/// 给会话挂载一个知识集(会话级粘连)。后续每条消息发送前自动检索注入。
#[tauri::command]
pub fn session_mount_collection(
    session_id: String,
    collection_id: i64,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<(), String> {
    // 完全门控:embedding 模型没就绪 → 知识库整体不可用,拒绝挂载。前端会置灰入口,
    // 这里是防绕过兜底(草稿态直调 / 旧前端 / 命令注入)。
    if !knowledge.semantic_ready() {
        return Err("embedding 模型未就绪,知识库暂不可用".to_string());
    }
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
pub fn session_set_mounted_collections(
    session_id: String,
    collections: Vec<crate::core::mode_state::MountedCollection>,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<Vec<crate::core::mode_state::MountedCollection>, String> {
    if collections.iter().any(|collection| collection.enabled) && !knowledge.semantic_ready() {
        return Err("embedding 模型未就绪,知识库暂不可用".to_string());
    }
    let mut normalized = Vec::new();
    for collection in collections {
        if collection.collection_id <= 0
            || normalized
                .iter()
                .any(|mounted: &crate::core::mode_state::MountedCollection| {
                    mounted.collection_id == collection.collection_id
                })
        {
            continue;
        }
        if knowledge
            .l1()
            .collection_name(collection.collection_id)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            continue;
        }
        normalized.push(collection);
    }
    let snapshot = store.set_mounted_collections(&session_id, normalized);
    publish_kb_mount_change(&app, &session_id, &snapshot);
    Ok(snapshot.collections)
}

fn ensure_collection_mountable(
    knowledge: &KnowledgeService,
    collection_id: i64,
) -> Result<(), String> {
    if collection_id <= 0 {
        return Err("知识集 id 无效".to_string());
    }
    if !knowledge.semantic_ready() {
        return Err("embedding 模型未就绪,知识库暂不可用".to_string());
    }
    if knowledge
        .l1()
        .collection_name(collection_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err("知识集不存在或已删除".to_string());
    }
    Ok(())
}

/// 在 SessionStore 的同一写锁内追加或重新启用一个知识集，避免跨端 read-modify-write 丢更新。
#[tauri::command]
pub fn session_add_mounted_collection(
    session_id: String,
    collection_id: i64,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<crate::core::mode_state::MountedCollectionsSnapshot, String> {
    ensure_collection_mountable(&knowledge, collection_id)?;
    let snapshot = store.add_mounted_collection(&session_id, collection_id);
    publish_kb_mount_change(&app, &session_id, &snapshot);
    Ok(snapshot)
}

/// 原子切换单个知识集的启用状态。停用不依赖模型或知识集仍然存在，以便清理陈旧状态。
#[tauri::command]
pub fn session_set_mounted_collection_enabled(
    session_id: String,
    collection_id: i64,
    enabled: bool,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<crate::core::mode_state::MountedCollectionsSnapshot, String> {
    if enabled {
        ensure_collection_mountable(&knowledge, collection_id)?;
    }
    let snapshot = store.set_mounted_collection_enabled(&session_id, collection_id, enabled);
    publish_kb_mount_change(&app, &session_id, &snapshot);
    Ok(snapshot)
}

/// 原子移除单个知识集；与其他端对不同知识集的并发操作可以安全合并。
#[tauri::command]
pub fn session_remove_mounted_collection(
    session_id: String,
    collection_id: i64,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> crate::core::mode_state::MountedCollectionsSnapshot {
    let snapshot = store.remove_mounted_collection(&session_id, collection_id);
    publish_kb_mount_change(&app, &session_id, &snapshot);
    snapshot
}

/// 摘下会话的知识集挂载。
#[tauri::command]
pub fn session_unmount_collection(
    session_id: String,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> crate::core::mode_state::MountedCollectionsSnapshot {
    let snapshot = store.set_mounted_collections(&session_id, Vec::new());
    publish_kb_mount_change(&app, &session_id, &snapshot);
    snapshot
}

fn publish_kb_mount_change(
    app: &AppHandle,
    session_id: &str,
    snapshot: &crate::core::mode_state::MountedCollectionsSnapshot,
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
    let _ = app.emit("remote_control:kb_mount_changed", payload.clone());
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
) -> Vec<crate::core::mode_state::MountedCollection> {
    store.mounted_collections(&session_id)
}

/// 带修订号读取挂载事实源，供多端拒绝乱序响应。
#[tauri::command]
pub fn session_mounted_collections_snapshot(
    session_id: String,
    store: State<'_, SessionStore>,
) -> crate::core::mode_state::MountedCollectionsSnapshot {
    store.mounted_collections_snapshot(&session_id)
}

use crate::features::knowledge as knowledge_domain;
use crate::features::knowledge::model_download as model_domain;
use knowledge_domain::*;
use model_domain::*;

sync_command_passthrough!(knowledge_domain, kb_start_scan(state: State<'_, KnowledgeService>, roots: Option<Vec<String>>) -> ScanState);
sync_command_passthrough!(knowledge_domain, kb_scan_status(state: State<'_, KnowledgeService>) -> ScanState);
sync_command_passthrough!(knowledge_domain, kb_cancel_scan(state: State<'_, KnowledgeService>));
async_command_passthrough!(knowledge_domain, kb_type_counts(state: State<'_, KnowledgeService>) -> Result<Vec<TypeCount>, String>);
async_command_passthrough!(knowledge_domain, kb_collection_list(state: State<'_, KnowledgeService>) -> Result<Vec<Collection>, String>);
async_command_passthrough!(knowledge_domain, kb_collection_create(state: State<'_, KnowledgeService>, name: String, category: Option<String>, description: Option<String>) -> Result<i64, String>);
async_command_passthrough!(knowledge_domain, kb_collection_update(state: State<'_, KnowledgeService>, id: i64, name: String, category: Option<String>, description: Option<String>) -> Result<(), String>);
async_command_passthrough!(knowledge_domain, kb_collection_delete(state: State<'_, KnowledgeService>, pool: State<'_, EnginePool>, id: i64) -> Result<(), String>);
sync_command_passthrough!(knowledge_domain, kb_collection_add_sources(state: State<'_, KnowledgeService>, collection_id: i64, paths: Vec<String>) -> IndexState);
sync_command_passthrough!(knowledge_domain, kb_index_status(state: State<'_, KnowledgeService>) -> IndexState);
sync_command_passthrough!(knowledge_domain, kb_index_cancel(state: State<'_, KnowledgeService>));
async_command_passthrough!(knowledge_domain, kb_documents(state: State<'_, KnowledgeService>, collection_id: i64, limit: Option<usize>) -> Result<Vec<Document>, String>);
async_command_passthrough!(knowledge_domain, kb_remove_document(state: State<'_, KnowledgeService>, pool: State<'_, EnginePool>, doc_id: i64) -> Result<(), String>);
sync_command_passthrough!(knowledge_domain, kb_embed_info(state: State<'_, KnowledgeService>) -> EmbedInfo);
async_command_passthrough!(knowledge_domain, kb_search(state: State<'_, KnowledgeService>, query: SearchQueryDto) -> Result<Vec<FileHit>, String>);
async_command_passthrough!(knowledge_domain, kb_stats(state: State<'_, KnowledgeService>) -> Result<Stats, String>);

sync_command_passthrough!(model_domain, kb_model_status(service: State<'_, KnowledgeService>) -> KbModelStatus);
sync_command_passthrough!(model_domain, kb_model_cancel());
async_command_passthrough!(model_domain, kb_model_load_after_first_frame(app: AppHandle, service: State<'_, KnowledgeService>, pool: State<'_, EnginePool>) -> Result<bool, String>);
async_command_passthrough!(model_domain, kb_model_download(app: AppHandle, service: State<'_, KnowledgeService>, pool: State<'_, EnginePool>) -> Result<KbModelStatus, String>);
use super::prelude::*;
