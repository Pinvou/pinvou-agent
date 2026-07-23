/// Agentic RAG 的 Self-RAG 自检引导:挂了知识集时每 turn prepend(动态状态走 per-turn
/// 注入)。引导模型自调 `kb_search`、严格基于检索结果作答、无依据就说不知道——治本地小
/// 模型"该查不查 → 凭记忆幻觉"(去掉注入式兜底后这是关键防线)。
#[tauri::command]
pub async fn ensure_llmapi_binding(
) -> Result<crate::features::llmapi_hub::models::EnsureLlmApiBindingResponse, String> {
    crate::features::llmapi_hub::provisioning::ensure_binding_for_current_user()
        .map_err(|err| err.to_tauri_error())
}

#[tauri::command]
pub async fn login_llmapi_user(
    username: String,
    password: String,
) -> Result<crate::features::llmapi_hub::models::EnsureLlmApiBindingResponse, String> {
    crate::features::llmapi_hub::provisioning::login_user_session_system(username, password)
        .map_err(|err| err.to_tauri_error())
}

#[tauri::command]
pub async fn save_llmapi_user_session(
    user_id: String,
    access_token: String,
) -> Result<crate::features::llmapi_hub::models::EnsureLlmApiBindingResponse, String> {
    crate::features::llmapi_hub::provisioning::save_user_session_system(user_id, access_token)
        .map_err(|err| err.to_tauri_error())
}

#[tauri::command]
pub async fn get_llmapi_status() -> Result<crate::features::llmapi_hub::models::LlmApiStatusResponse, String>
{
    let started_at = std::time::Instant::now();
    log::info!("[llmapi_hub][commands] get_llmapi_status start");
    let remote = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        tokio::task::spawn_blocking(crate::features::llmapi_hub::provisioning::status_for_current_user_system),
    )
    .await;

    match remote {
        Ok(Ok(Ok(status))) => {
            log::info!(
                "[llmapi_hub][commands] get_llmapi_status ok elapsed_ms={} backend_user_exists={} status={:?} backend_username={} backend_display_name={} limit={} used={} remaining={}",
                started_at.elapsed().as_millis(),
                status.backend_user_exists,
                status.provisioning_status,
                status.backend_username.as_deref().unwrap_or(""),
                status.backend_display_name.as_deref().unwrap_or(""),
                status.quota.as_ref().map(|q| q.limit_tokens).unwrap_or(0),
                status.quota.as_ref().map(|q| q.used_tokens).unwrap_or(0),
                status.quota.as_ref().map(|q| q.remaining_tokens).unwrap_or(0)
            );
            Ok(status)
        }
        Ok(Ok(Err(err))) => {
            log::warn!(
                "[llmapi_hub][commands] get_llmapi_status backend failed elapsed_ms={} code={:?} retryable={} message={}",
                started_at.elapsed().as_millis(),
                err.code,
                err.retryable,
                err.message
            );
            crate::features::llmapi_hub::provisioning::unavailable_status_for_current_user_system(
                Some(err.code),
                Some(err.message),
            )
            .map_err(|err| err.to_tauri_error())
        }
        Ok(Err(err)) => {
            log::warn!(
                "[llmapi_hub][commands] get_llmapi_status task join failed elapsed_ms={} error={err}",
                started_at.elapsed().as_millis()
            );
            crate::features::llmapi_hub::provisioning::unavailable_status_for_current_user_system(
                Some(crate::features::llmapi_hub::models::LlmApiErrorCode::Unavailable),
                Some(format!("后台状态查询任务失败: {err}")),
            )
                .map_err(|err| err.to_tauri_error())
        }
        Err(_) => {
            log::warn!(
                "[llmapi_hub][commands] get_llmapi_status backend refresh timed out elapsed_ms={}; returning unavailable status",
                started_at.elapsed().as_millis()
            );
            crate::features::llmapi_hub::provisioning::unavailable_status_for_current_user_system(
                Some(crate::features::llmapi_hub::models::LlmApiErrorCode::ServiceUnreachable),
                Some("后台状态查询超时".to_string()),
            )
                .map_err(|err| err.to_tauri_error())
        }
    }
}
#[tauri::command]
pub async fn get_llmapi_models(
) -> Result<crate::features::llmapi_hub::models::BuiltinLlmApiModelsResponse, String> {
    let started_at = std::time::Instant::now();
    log::info!("[llmapi_hub][commands] get_llmapi_models start");
    let result = tokio::task::spawn_blocking(
        crate::features::llmapi_hub::provisioning::available_models_system,
    )
    .await
    .map_err(|err| format!("get_llmapi_models task join failed: {err}"))?;
    match result {
        Ok(models) => {
            log::info!(
                "[llmapi_hub][commands] get_llmapi_models ok elapsed_ms={} count={} default_model={}",
                started_at.elapsed().as_millis(),
                models.available_models.len(),
                models.default_model
            );
            Ok(models)
        }
        Err(err) => {
            log::warn!(
                "[llmapi_hub][commands] get_llmapi_models failed elapsed_ms={} code={:?} retryable={} message={}",
                started_at.elapsed().as_millis(),
                err.code,
                err.retryable,
                err.message
            );
            Err(err.to_tauri_error())
        }
    }
}

#[tauri::command]
pub async fn set_llmapi_default_model(
    model: String,
) -> Result<crate::features::llmapi_hub::models::BuiltinLlmApiModelsResponse, String> {
    crate::features::llmapi_hub::provisioning::set_default_model_system(&model)
        .map_err(|err| err.to_tauri_error())
}

#[tauri::command]
pub async fn retry_llmapi_provisioning(
    pinvou_user_id: String,
    device_binding_id: String,
) -> Result<crate::features::llmapi_hub::models::EnsureLlmApiBindingResponse, String> {
    crate::features::llmapi_hub::provisioning::retry_binding_system(pinvou_user_id, device_binding_id)
        .map_err(|err| err.to_tauri_error())
}

#[derive(Debug, Clone, Serialize)]
pub struct SetLlmApiUserEnabledResponse {
    pub pinvou_user_id: String,
    pub enabled: bool,
}

#[tauri::command]
pub async fn set_llmapi_user_enabled(
    pinvou_user_id: String,
    enabled: bool,
) -> Result<SetLlmApiUserEnabledResponse, String> {
    let binding =
        crate::features::llmapi_hub::provisioning::set_user_enabled_system(&pinvou_user_id, enabled)
            .map_err(|err| err.to_tauri_error())?;
    Ok(SetLlmApiUserEnabledResponse {
        pinvou_user_id: binding.pinvou_user_id,
        enabled: binding.enabled,
    })
}

#[tauri::command]
pub async fn get_llmapi_admin_overview(
    query: Option<String>,
    status: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<crate::features::llmapi_hub::models::LlmApiAdminOverviewResponse, String> {
    let status = status
        .as_deref()
        .map(parse_llmapi_provisioning_status)
        .transpose()?;
    crate::features::llmapi_hub::provisioning::admin_overview_system(query, status, limit, offset)
        .map_err(|err| err.to_tauri_error())
}

fn parse_llmapi_provisioning_status(
    value: &str,
) -> Result<crate::features::llmapi_hub::models::ProvisioningStatus, String> {
    match value {
        "not_started" => Ok(crate::features::llmapi_hub::models::ProvisioningStatus::NotStarted),
        "querying_user" | "creating_user" => {
            Ok(crate::features::llmapi_hub::models::ProvisioningStatus::QueryingUser)
        }
        "creating_token" => Ok(crate::features::llmapi_hub::models::ProvisioningStatus::CreatingToken),
        "configuring_policy" => {
            Ok(crate::features::llmapi_hub::models::ProvisioningStatus::ConfiguringPolicy)
        }
        "ready" => Ok(crate::features::llmapi_hub::models::ProvisioningStatus::Ready),
        "failed" => Ok(crate::features::llmapi_hub::models::ProvisioningStatus::Failed),
        "disabled" => Ok(crate::features::llmapi_hub::models::ProvisioningStatus::Disabled),
        _ => Err(format!("invalid LLM API Hub provisioning status: {value}")),
    }
}
use super::prelude::*;
