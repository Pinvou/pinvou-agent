/// pinvou3 工具开关(按会话类型 scope 持久):设置当前被关掉的连接器
/// (connector_ids = 市场工具 id)。落盘 → 推算成模型可见工具全名广播给所有在跑
/// 引擎 → 隐藏这些工具。空 = 全开。
/// 持久:用户关一次,该 scope 所有新对话/新窗口都继承,直到手动开回。
/// `scope` = "plain"(普通会话,缺省)或 "code"(原生代码会话);两个 scope 独立。
#[tauri::command]
pub async fn set_disabled_connectors(
    connector_ids: Vec<String>,
    scope: Option<String>,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    let scope = parse_connector_scope(scope.as_deref())?;
    crate::features::marketplace::apply_disabled_connectors_for(scope, connector_ids).await?;
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

/// pinvou3 工具开关:读某 scope 被禁用的连接器 id 列表(前端启动时加载,初始化开关状态)。
/// `scope` = "plain"(缺省)或 "code"。
#[tauri::command]
pub async fn get_disabled_connectors(scope: Option<String>) -> Result<Vec<String>, String> {
    let scope = parse_connector_scope(scope.as_deref())?;
    Ok(crate::features::marketplace::load_disabled_connectors_for(
        scope,
    ))
}

/// 解析前端传入的 scope:缺省/空 = plain;"plain"/"code" 显式对应两个 scope;
/// 其余未识别的非空字符串返回错误(前端笔误直接报错,不静默回退 plain)。
fn parse_connector_scope(
    scope: Option<&str>,
) -> Result<crate::features::marketplace::ConnectorScope, String> {
    use crate::features::marketplace::ConnectorScope;
    match scope {
        Some("code") => Ok(ConnectorScope::Code),
        Some("plain") => Ok(ConnectorScope::Plain),
        Some(other) if !other.trim().is_empty() => Err(format!(
            "未知的连接器 scope '{other}'，仅支持 \"plain\"(缺省)或 \"code\""
        )),
        _ => Ok(ConnectorScope::Plain),
    }
}

use crate::features::connectors::{
    connector_cli as connector_cli_domain, dingtalk as dingtalk_domain, feishu as feishu_domain,
    ima as ima_domain, tmeet as tmeet_domain, wecom as wecom_domain,
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

async_command_passthrough!(tmeet_domain, tmeet_ensure_cli() -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_status() -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_logout() -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_apply_skills() -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, set_tmeet_enabled(enabled: bool) -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_skills_state() -> Result<Value, String>);

async_command_passthrough!(ima_domain, ima_status() -> Result<Value, String>);
async_command_passthrough!(ima_domain, ima_connect(client_id: String, api_key: String) -> Result<Value, String>);
async_command_passthrough!(ima_domain, ima_logout() -> Result<Value, String>);
use super::prelude::*;

#[cfg(test)]
mod tests {
    use super::parse_connector_scope;
    use crate::features::marketplace::ConnectorScope;

    #[test]
    fn parse_connector_scope_defaults_to_plain() {
        assert_eq!(parse_connector_scope(None).unwrap(), ConnectorScope::Plain);
        assert_eq!(parse_connector_scope(Some("")).unwrap(), ConnectorScope::Plain);
        assert_eq!(
            parse_connector_scope(Some("plain")).unwrap(),
            ConnectorScope::Plain
        );
    }

    #[test]
    fn parse_connector_scope_accepts_code() {
        assert_eq!(
            parse_connector_scope(Some("code")).unwrap(),
            ConnectorScope::Code
        );
    }

    #[test]
    fn parse_connector_scope_rejects_unknown_values() {
        let err = parse_connector_scope(Some("cdoe")).unwrap_err();
        assert!(err.contains("cdoe"), "错误应回显原始输入: {err}");
        assert!(parse_connector_scope(Some("CODE")).is_err());
        assert!(parse_connector_scope(Some("global")).is_err());
    }
}
