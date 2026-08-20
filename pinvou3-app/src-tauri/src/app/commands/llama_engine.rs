//! 本地多模态引擎命令（薄传输层：透传 features::llama_engine 域函数）。

use super::prelude::*;
use crate::features::llama_engine as llama_engine_domain;
use llama_engine_domain::*;

sync_command_passthrough!(llama_engine_domain, llama_engine_status() -> LlamaEngineStatus);
async_command_passthrough!(llama_engine_domain, llama_engine_install_engine(app: AppHandle) -> Result<(), String>);
async_command_passthrough!(llama_engine_domain, llama_engine_install_model(app: AppHandle, model: String) -> Result<(), String>);
sync_command_passthrough!(llama_engine_domain, llama_engine_cancel_download());
sync_command_passthrough!(llama_engine_domain, llama_engine_stop());

/// 手动启动引擎（设置页按钮）。成功后对全部 saved_models bump revision：
/// 引擎启动前已 spawn 的旧会话据此重建 EngineConfig 快照，本轮起即可用
/// 本地 image_analyze（快照语义修复，见 chat.rs ensure_local_engine_ready）。
#[tauri::command]
pub async fn llama_engine_start(
    app: AppHandle,
    model: String,
    device: String,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    let was_running = llama_engine_domain::server::runtime_snapshot().phase == "running";
    let result = llama_engine_domain::llama_engine_start(app, model, device).await;
    if result.is_ok() && !was_running {
        let prefs = UserPrefs::load();
        for saved in &prefs.advanced.saved_models {
            pool.mark_model_updated(&saved.id);
        }
    }
    result
}
