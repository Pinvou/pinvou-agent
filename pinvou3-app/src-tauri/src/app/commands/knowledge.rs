pub(super) fn build_kb_agentic_guide(collection_name: Option<&str>) -> String {
    let title = collection_name.unwrap_or("本地知识集");
    format!(
        "<system-reminder>\n\
         本会话挂载了知识集《{title}》。涉及用户本地资料/文档的问题,你**必须先调用 \
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
    let _ = app.emit(
        "remote_control:kb_mount_changed",
        serde_json::json!({ "session_id": session_id, "collection_id": collection_id }),
    );
    broadcast_kb_mount_to_mobile(&app, &session_id, Some(collection_id));
    Ok(())
}

/// 摘下会话的知识集挂载。
#[tauri::command]
pub fn session_unmount_collection(
    session_id: String,
    store: State<'_, SessionStore>,
    app: AppHandle,
) {
    store.set_mounted_collection(&session_id, None);
    let _ = app.emit(
        "remote_control:kb_mount_changed",
        serde_json::json!({ "session_id": session_id, "collection_id": null }),
    );
    broadcast_kb_mount_to_mobile(&app, &session_id, None);
}

fn broadcast_kb_mount_to_mobile(app: &AppHandle, session_id: &str, collection_id: Option<i64>) {
    let payload = serde_json::json!({
        "session_id": session_id,
        "collection_id": collection_id,
    });
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
sync_command_passthrough!(knowledge_domain, kb_index_resume(state: State<'_, KnowledgeService>, job_id: String) -> Result<IndexState, String>);
sync_command_passthrough!(knowledge_domain, kb_index_retry_file(state: State<'_, KnowledgeService>, job_id: String, item_id: i64) -> Result<IndexState, String>);
async_command_passthrough!(knowledge_domain, kb_documents(state: State<'_, KnowledgeService>, collection_id: i64, limit: Option<usize>) -> Result<Vec<Document>, String>);
async_command_passthrough!(knowledge_domain, kb_remove_document(state: State<'_, KnowledgeService>, pool: State<'_, EnginePool>, doc_id: i64) -> Result<(), String>);
sync_command_passthrough!(knowledge_domain, kb_embed_info(state: State<'_, KnowledgeService>) -> EmbedInfo);
async_command_passthrough!(knowledge_domain, kb_search(state: State<'_, KnowledgeService>, query: SearchQueryDto) -> Result<Vec<FileHit>, String>);
async_command_passthrough!(knowledge_domain, kb_stats(state: State<'_, KnowledgeService>) -> Result<Stats, String>);

sync_command_passthrough!(model_domain, kb_model_status() -> KbModelStatus);
sync_command_passthrough!(model_domain, kb_model_cancel());
async_command_passthrough!(model_domain, kb_model_load_after_first_frame(app: AppHandle, service: State<'_, KnowledgeService>, pool: State<'_, EnginePool>) -> Result<bool, String>);
async_command_passthrough!(model_domain, kb_model_download(app: AppHandle, service: State<'_, KnowledgeService>, pool: State<'_, EnginePool>) -> Result<KbModelStatus, String>);
use super::prelude::*;
