//! ACP Agent 第三方 Provider（中转）管理的 Tauri 命令。
//!
//! 传输边界与参数校验；Provider 读写、写入器编排与会话重启均由
//! `features::codex_acp` 领域模块负责。

use tauri::State;

use crate::features::codex_acp::{
    AcpPool, AcpProvidersView, CodexAcpSessionInfo, CodexAcpStatus, ImportResult, ProviderRecord,
    ProviderWireApi,
};

fn parse_wire_api(value: &str) -> Result<ProviderWireApi, String> {
    ProviderWireApi::parse(Some(value)).map_err(|error| format!("{error:#}"))
}

fn parse_key_action(
    value: &str,
) -> Result<crate::platform::credential_store::CredentialEditAction, String> {
    match value {
        "replace" => Ok(crate::platform::credential_store::CredentialEditAction::Replace),
        "keep" => Ok(crate::platform::credential_store::CredentialEditAction::KeepExisting),
        "delete" => Ok(crate::platform::credential_store::CredentialEditAction::Delete),
        other => Err(format!("不支持的 key 动作: {other}")),
    }
}

#[tauri::command]
pub fn list_acp_providers(
    agent: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<AcpProvidersView, String> {
    acp_pool
        .list_acp_providers(&agent)
        .map_err(|error| format!("读取 Provider 列表失败: {error:#}"))
}

#[tauri::command]
pub async fn save_acp_provider(
    agent: String,
    provider_id: Option<String>,
    name: String,
    base_url: String,
    model: Option<String>,
    model_slots: Option<std::collections::HashMap<String, String>>,
    context_window: Option<i64>,
    wire_api: String,
    api_key: Option<String>,
    api_key_action: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<ProviderRecord, String> {
    let wire_api = parse_wire_api(&wire_api)?;
    let api_key_action = parse_key_action(&api_key_action)?;
    acp_pool
        .save_acp_provider(
            &agent,
            provider_id.as_deref(),
            name,
            base_url,
            model,
            model_slots,
            context_window,
            wire_api,
            api_key,
            api_key_action,
        )
        .await
        .map_err(|error| format!("保存 Provider 失败: {error:#}"))
}

#[tauri::command]
pub async fn delete_acp_provider(
    agent: String,
    provider_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    let pool = acp_pool.inner().clone();
    pool.delete_acp_provider(&agent, &provider_id)
        .await
        .map_err(|error| format!("删除 Provider 失败: {error:#}"))
}

#[tauri::command]
pub async fn switch_acp_provider(
    agent: String,
    provider_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    let pool = acp_pool.inner().clone();
    pool.switch_acp_provider(&agent, &provider_id)
        .await
        .map_err(|error| format!("切换 Provider 失败: {error:#}"))
}

#[tauri::command]
pub async fn switch_acp_provider_official(
    agent: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    let pool = acp_pool.inner().clone();
    pool.switch_acp_provider_official(&agent)
        .await
        .map_err(|error| format!("恢复官方登录失败: {error:#}"))
}

#[tauri::command]
pub async fn uninstall_acp_agent(
    agent: String,
    cleanup: Option<bool>,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    let pool = acp_pool.inner().clone();
    pool.uninstall_acp_agent(&agent, cleanup.unwrap_or(false))
        .await
        .map_err(|error| format!("卸载 ACP Agent 失败: {error:#}"))
}

/// 取消正在进行的 Agent CLI 安装（按登记的 pid 杀安装进程树）。
#[tauri::command]
pub async fn cancel_acp_agent_install(
    agent: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    let pool = acp_pool.inner().clone();
    pool.cancel_agent_install(&agent)
        .await
        .map_err(|error| format!("取消安装失败: {error:#}"))
}

/// 读取 Provider 的 API key（明文，仅编辑弹窗「显示密钥」时按需调用；
/// 列表/卡片永不回传）。无 key 时返回 null。
#[tauri::command]
pub fn get_acp_provider_key(
    agent: String,
    provider_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Option<String>, String> {
    acp_pool
        .get_acp_provider_key(&agent, &provider_id)
        .map_err(|error| format!("读取 Provider key 失败: {error:#}"))
}

/// 官方登出（codex `logout` / claude `auth logout`；kimi 不支持非交互登出）。
#[tauri::command]
pub async fn logout_acp_agent(
    agent: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    let pool = acp_pool.inner().clone();
    pool.logout_acp_agent(&agent)
        .await
        .map_err(|error| format!("登出失败: {error:#}"))
}

#[tauri::command]
pub fn export_acp_providers(agent: String, acp_pool: State<'_, AcpPool>) -> Result<String, String> {
    acp_pool
        .export_acp_providers(&agent)
        .map_err(|error| format!("导出 Provider 失败: {error:#}"))
}

#[tauri::command]
pub fn import_acp_providers(
    agent: String,
    json: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<ImportResult, String> {
    acp_pool
        .import_acp_providers(&agent, &json)
        .map_err(|error| format!("导入 Provider 失败: {error:#}"))
}

/// 一次性模型探针：切换 Provider 后由对话页草稿态触发，真实连接一次 ACP
/// 拉取新 Provider 的模型/配置上报；探针会话即用即弃（后端负责清理进程、
/// store 记录与临时目录），失败时前端回退到 reseed 占位快照。
#[tauri::command]
pub async fn probe_acp_agent_models(
    agent: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpSessionInfo, String> {
    let pool = acp_pool.inner().clone();
    pool.probe_agent_model_options(&agent)
        .await
        .map_err(|error| format!("模型探针失败: {error:#}"))
}

/// 会话级 Provider 覆盖（F11）。
#[tauri::command]
pub async fn set_codex_acp_session_provider(
    session_id: String,
    provider_id: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpSessionInfo, String> {
    let pool = acp_pool.inner().clone();
    pool.set_acp_session_provider(&session_id, provider_id)
        .await
        .map_err(|error| format!("设置会话 Provider 失败: {error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wire_and_key_action() {
        assert_eq!(parse_wire_api("openai").unwrap(), ProviderWireApi::Openai);
        assert_eq!(
            parse_wire_api("anthropic").unwrap(),
            ProviderWireApi::Anthropic
        );
        assert!(parse_wire_api("bogus").is_err());
        assert!(parse_key_action("replace").is_ok());
        assert!(parse_key_action("keep").is_ok());
        assert!(parse_key_action("delete").is_ok());
        assert!(parse_key_action("bogus").is_err());
    }
}
