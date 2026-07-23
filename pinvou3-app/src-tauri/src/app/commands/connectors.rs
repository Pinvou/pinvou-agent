/// pinvou3 工具开关(全局持久):设置当前被关掉的连接器(connector_ids = 市场工具 id)。
/// 落盘 → 推算成模型可见工具全名广播给所有在跑引擎 → 隐藏这些工具。空 = 全开。
/// 持久:用户关一次,所有新对话/新窗口都继承,直到手动开回。
#[tauri::command]
pub async fn set_disabled_connectors(
    connector_ids: Vec<String>,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    crate::features::marketplace::apply_disabled_connectors(connector_ids).await?;
    pool.refresh_disallowed_tools().await;
    let payload = serde_json::json!({});
    let _ = app.emit("remote_control:tools_changed", payload.clone());
    crate::features::remote_control::forward_app_event(
        &app,
        "remote_control:tools_changed",
        payload,
    );
    Ok(())
}

/// pinvou3 工具开关:读全局被禁用的连接器 id 列表(前端启动时加载,初始化开关状态)。
#[tauri::command]
pub async fn get_disabled_connectors() -> Result<Vec<String>, String> {
    Ok(crate::features::marketplace::load_disabled_connectors())
}

use crate::features::connectors::{
    connector_cli as connector_cli_domain, dingtalk as dingtalk_domain, eip as eip_domain,
    feishu as feishu_domain, wecom as wecom_domain, zhidao as zhidao_domain,
};
use connector_cli_domain::*;
use serde_json::Value;

async_command_passthrough!(connector_cli_domain, refresh_connector_auth_gates() -> Result<ConnectorAuthGateRefresh, String>);

async_command_passthrough!(feishu_domain, feishu_ensure_cli() -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_status() -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_logout() -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_apply_skills() -> Result<Value, String>);
async_command_passthrough!(feishu_domain, set_feishu_enabled(enabled: bool) -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_skills_state() -> Result<Value, String>);

async_command_passthrough!(wecom_domain, wecom_ensure_cli() -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_status() -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_logout() -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_apply_skills() -> Result<Value, String>);
async_command_passthrough!(wecom_domain, set_wecom_enabled(enabled: bool) -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_skills_state() -> Result<Value, String>);

async_command_passthrough!(dingtalk_domain, dingtalk_ensure_cli() -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_status() -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_logout() -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_apply_skills() -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, set_dingtalk_enabled(enabled: bool) -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_skills_state() -> Result<Value, String>);

async_command_passthrough!(eip_domain, eip_status() -> Result<Value, String>);
async_command_passthrough!(eip_domain, eip_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(eip_domain, eip_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(eip_domain, eip_logout() -> Result<Value, String>);

async_command_passthrough!(zhidao_domain, zhidao_status() -> Result<Value, String>);
async_command_passthrough!(zhidao_domain, zhidao_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(zhidao_domain, zhidao_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(zhidao_domain, zhidao_logout() -> Result<Value, String>);
use super::prelude::*;
