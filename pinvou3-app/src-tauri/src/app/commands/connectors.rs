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
    // 连接器禁用影响其 companion skills 的可见性（组合目录排除集变化）：
    // 重写在线会话组合目录 + 热刷工具白名单。
    pool.refresh_live_sessions_skills().await;
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

// ---------------------------------------------------------------------------
// 技能开关（按会话类型 scope 独立持久，skill 双 scope 治理）
// ---------------------------------------------------------------------------

/// pinvou3 技能开关：写某 scope 被禁用的技能 id 列表（市场 id）。落盘
/// `~/.pinvou3/disabled_skills.json` → 重写该 scope 在线会话的组合目录（下一轮
/// prompt 即生效）→ 热刷工具白名单（组合目录空/非空会改变 `load_skill` 的隐藏
/// 判定）。`scope` = "plain"(缺省)或 "code"。
#[tauri::command]
pub async fn set_disabled_skills(
    skill_ids: Vec<String>,
    scope: Option<String>,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    let scope = parse_connector_scope(scope.as_deref())?;
    let ids = skill_ids.clone();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::skill_scope::save_disabled_skills_for(scope, &ids);
    })
    .await
    .map_err(|e| format!("set_disabled_skills join: {e}"))?;
    // 事件驱动时机（§2.3.2）：该 scope 在线会话组合目录增量重写，下一轮 prompt 生效。
    pool.refresh_live_sessions_skills().await;
    // load_skill 隐藏判定随目录空/非空变化，热刷 disallowed_tools 与 UI 事件。
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

/// pinvou3 技能开关：读某 scope 被禁用的技能 id 列表。code scope 未初始化时
/// 返回全部已安装技能 id（默认全禁，显式开启），前端据此渲染开关状态。
#[tauri::command]
pub async fn get_disabled_skills(scope: Option<String>) -> Result<Vec<String>, String> {
    let scope = parse_connector_scope(scope.as_deref())?;
    Ok(crate::features::marketplace::skill_scope::load_disabled_skills_for(scope))
}

/// 项目级 skills 开关（默认关，§2.4）。开启后绑项目的 code 会话组合目录额外
/// 包含项目 `.agents/skills` 等目录——项目内文本是 prompt-injection 面，前端
/// 在开启路径展示注入风险警告。
#[tauri::command]
pub async fn set_project_skills_enabled(
    enabled: bool,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    crate::features::marketplace::skill_scope::set_project_skills_enabled(enabled);
    // 开关影响 code 会话组合目录：重写在线会话 + 热刷 load_skill 隐藏判定。
    pool.refresh_live_sessions_skills().await;
    pool.refresh_disallowed_tools().await;
    // 广播工具变更：项目级 skills 开关影响 code 会话组合目录，其它窗口/实例
    // 需借此事件刷新开关状态（与 set_disabled_skills 对齐）。
    let payload = serde_json::json!({});
    let _ = app.emit("remote_control:tools_changed", payload.clone());
    crate::features::remote_control::forward_app_event(
        &app,
        "remote_control:tools_changed",
        payload,
    );
    Ok(())
}

/// 项目级 skills 开关状态（默认关）。
#[tauri::command]
pub async fn get_project_skills_enabled() -> Result<bool, String> {
    Ok(crate::features::marketplace::skill_scope::project_skills_enabled())
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

fn track_cli_install(app: &AppHandle, tool_key: &str, tool_name: &str) {
    crate::features::behavior_telemetry::track(
        app,
        crate::features::behavior_telemetry::BehaviorEvent::new("tool_install_completed")
            .tool(tool_key, tool_name, "cli")
            .success(true),
    );
}

fn was_new_cli_install(result: &Value) -> bool {
    result
        .get("already")
        .and_then(Value::as_bool)
        .is_some_and(|already| !already)
}

#[tauri::command]
pub async fn feishu_ensure_cli(app: AppHandle) -> Result<Value, String> {
    let result = feishu_domain::feishu_ensure_cli().await?;
    if was_new_cli_install(&result) {
        track_cli_install(&app, "feishu", "飞书");
    }
    Ok(result)
}
async_command_passthrough!(feishu_domain, feishu_status() -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_logout() -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_apply_skills() -> Result<Value, String>);
async_command_passthrough!(feishu_domain, set_feishu_enabled(enabled: bool) -> Result<Value, String>);
async_command_passthrough!(feishu_domain, feishu_skills_state() -> Result<Value, String>);

#[tauri::command]
pub async fn wecom_ensure_cli(app: AppHandle) -> Result<Value, String> {
    let result = wecom_domain::wecom_ensure_cli().await?;
    if was_new_cli_install(&result) {
        track_cli_install(&app, "wecom", "企业微信");
    }
    Ok(result)
}
async_command_passthrough!(wecom_domain, wecom_status() -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_logout() -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_apply_skills() -> Result<Value, String>);
async_command_passthrough!(wecom_domain, set_wecom_enabled(enabled: bool) -> Result<Value, String>);
async_command_passthrough!(wecom_domain, wecom_skills_state() -> Result<Value, String>);

#[tauri::command]
pub async fn dingtalk_ensure_cli(app: AppHandle) -> Result<Value, String> {
    let result = dingtalk_domain::dingtalk_ensure_cli().await?;
    if was_new_cli_install(&result) {
        track_cli_install(&app, "dingtalk", "钉钉");
    }
    Ok(result)
}
async_command_passthrough!(dingtalk_domain, dingtalk_status() -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_logout() -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_apply_skills() -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, set_dingtalk_enabled(enabled: bool) -> Result<Value, String>);
async_command_passthrough!(dingtalk_domain, dingtalk_skills_state() -> Result<Value, String>);

#[tauri::command]
pub async fn tmeet_ensure_cli(app: AppHandle) -> Result<Value, String> {
    let result = tmeet_domain::tmeet_ensure_cli().await?;
    if was_new_cli_install(&result) {
        track_cli_install(&app, "tmeet", "腾讯会议");
    }
    Ok(result)
}
async_command_passthrough!(tmeet_domain, tmeet_status() -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_connect_begin(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_cancel(app: AppHandle) -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_logout() -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_apply_skills() -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, set_tmeet_enabled(enabled: bool) -> Result<Value, String>);
async_command_passthrough!(tmeet_domain, tmeet_skills_state() -> Result<Value, String>);

async_command_passthrough!(ima_domain, ima_status() -> Result<Value, String>);

/// ima 连接成功会安装配套技能 ima-skills（domain 层落盘）→ 重写在线会话组合目录
/// （skill 双 scope 治理事件驱动时机）。失败时技能未装上，不重写。
#[tauri::command]
pub async fn ima_connect(
    client_id: String,
    api_key: String,
    pool: State<'_, EnginePool>,
) -> Result<Value, String> {
    let result = ima_domain::ima_connect(client_id, api_key).await?;
    pool.refresh_live_sessions_skills().await;
    Ok(result)
}

/// ima 退出会卸载配套技能 ima-skills（domain 层落盘）→ 重写在线会话组合目录。
#[tauri::command]
pub async fn ima_logout(pool: State<'_, EnginePool>) -> Result<Value, String> {
    let result = ima_domain::ima_logout().await?;
    pool.refresh_live_sessions_skills().await;
    Ok(result)
}
use super::prelude::*;

#[cfg(test)]
mod tests {
    use super::parse_connector_scope;
    use crate::features::marketplace::ConnectorScope;

    #[test]
    fn parse_connector_scope_defaults_to_plain() {
        assert_eq!(parse_connector_scope(None).unwrap(), ConnectorScope::Plain);
        assert_eq!(
            parse_connector_scope(Some("")).unwrap(),
            ConnectorScope::Plain
        );
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
