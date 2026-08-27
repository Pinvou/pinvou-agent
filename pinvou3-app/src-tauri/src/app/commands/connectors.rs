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
    // 重写在线会话组合目录 + 热刷工具白名单 + 热刷 CLI 硬拦截规则集（execpolicy）。
    pool.refresh_live_sessions_skills().await;
    pool.refresh_disallowed_tools().await;
    pool.refresh_permission_rulesets().await;
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

/// 商店「管理可见性」：写某 scope 被「不可见」的包 id 列表。控制 composer 列表显隐 +
/// 底座可用集（union 开关关+不可见）。与开关（set_disabled_connectors）正交。
#[tauri::command]
pub async fn set_bundle_visibility(
    bundle_ids: Vec<String>,
    scope: Option<String>,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    let scope = parse_connector_scope(scope.as_deref())?;
    let ids = bundle_ids.clone();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::save_hidden_bundles_for(scope, &ids);
    })
    .await
    .map_err(|e| format!("set_bundle_visibility join: {e}"))?;
    pool.refresh_live_sessions_skills().await;
    pool.refresh_disallowed_tools().await;
    pool.refresh_permission_rulesets().await;
    let payload = serde_json::json!({});
    let _ = app.emit("remote_control:tools_changed", payload.clone());
    crate::features::remote_control::forward_app_event(
        &app,
        "remote_control:tools_changed",
        payload,
    );
    Ok(())
}

/// 商店「管理可见性」：读某 scope 被「不可见」的包 id 列表（可见性预过滤，非开关）。
/// 缺省空 = 全可见。`scope` = "plain"(缺省)或 "code"。
#[tauri::command]
pub async fn get_bundle_visibility(scope: Option<String>) -> Result<Vec<String>, String> {
    let scope = parse_connector_scope(scope.as_deref())?;
    Ok(crate::features::marketplace::load_hidden_bundles_for(scope))
}

// ---------------------------------------------------------------------------
// 技能开关（按会话类型 scope 独立持久，skill 双 scope 治理）
// ---------------------------------------------------------------------------

/// pinvou3 技能开关：写某 scope 被禁用的技能 id 列表（市场 id）。落盘
/// `~/.pinvou3/disabled_bundles.json`（scope 收敛后与连接器开关同一文件）→ 重写该 scope 在线会话的组合目录（下一轮
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
    // 技能开关影响 execpolicy 规则集（skill denied_prefixes 与组合目录派生 deny），需双向热刷
    pool.refresh_permission_rulesets().await;
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
    // 同步 execpolicy 规则集：项目级 skills 重新纳入 deny/allow 集合。
    pool.refresh_permission_rulesets().await;
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

/// 解析前端传入的 scope:缺省/空 = plain;已注册模式名(`SessionMode` 的
/// kebab-case,当前 "plain"/"code")显式对应;其余未识别的非空字符串返回错误
/// (前端笔误直接报错,不静默回退 plain)。前端协议字符串不变。
fn parse_connector_scope(
    scope: Option<&str>,
) -> Result<crate::features::marketplace::ConnectorScope, String> {
    use crate::core::session_mode::SessionMode;
    match scope {
        Some(s) if !s.trim().is_empty() => SessionMode::from_scope_str(s)
            .ok_or_else(|| format!("未知的连接器 scope '{s}'，仅支持 \"plain\"(缺省)或 \"code\"")),
        _ => Ok(SessionMode::Plain),
    }
}

use crate::features::connectors::{
    connector_cli as connector_cli_domain, ima as ima_domain, registry as connectors_registry,
};
use connector_cli_domain::*;
use serde_json::Value;

/// 连接/断开后的技能门控刷新 + execpolicy 规则集热刷（二轮评审 M-6：connect 路径
/// 规则集不热刷会让在跑引擎对刚连接连接器的技能脚本/CLI 拦截过期）。
#[tauri::command]
pub async fn refresh_connector_auth_gates(
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<ConnectorAuthGateRefresh, String> {
    let result = connector_cli_domain::refresh_connector_auth_gates().await?;
    // 技能目录可能已增删 → 技能脚本 deny 规则（code 默认全禁已装技能）要按新目录重算。
    pool.refresh_permission_rulesets().await;
    Ok(result)
}

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

// ---------------------------------------------------------------------------
// 通用连接器命令（阶段 3a：4 连接器 × 8 硬编码命令原子替换为按 id 分派——
// 内置 4 连接器到既有实现、声明式 Upload 包到契约驱动通用编排器，分派见
// `features/connectors/registry`。事件契约统一为 `connector:event`）。
// ---------------------------------------------------------------------------

/// 引导：确保连接器 CLI 就位（内置 = lock 表下载校验；声明式包 = 声明下载校验；
/// tmeet 的 npm 路径保留在其 ensure 实现内）。
#[tauri::command]
pub async fn connector_ensure_cli(id: String, app: AppHandle) -> Result<Value, String> {
    let result = connectors_registry::ensure_cli(&id).await?;
    if was_new_cli_install(&result) {
        let name = connectors_registry::display_name(&id);
        track_cli_install(&app, &id, &name);
    }
    Ok(result)
}

/// 查询连接器连接状态（只回布尔/结构化态，不带身份信息）。
#[tauri::command]
pub async fn connector_status(id: String) -> Result<Value, String> {
    connectors_registry::status(&id).await
}

/// 开始连接：立即返回 `{started:true}`（manual 包返回 `{started:false, mode:"manual"}`），
/// 进度经统一事件 `connector:event` 上报。
#[tauri::command]
pub async fn connector_connect_begin(id: String, app: AppHandle) -> Result<Value, String> {
    connectors_registry::connect_begin(&app, &id).await
}

/// 取消连接：置取消标志 + tree-kill 当前长驻子进程。
#[tauri::command]
pub async fn connector_cancel(id: String, app: AppHandle) -> Result<Value, String> {
    connectors_registry::cancel(&app, &id).await
}

/// 断开连接器（清授权；声明式包 CLI 无 logout 能力时降级为仅清本地登记）。
#[tauri::command]
pub async fn connector_logout(id: String) -> Result<Value, String> {
    connectors_registry::logout(&id).await
}

/// 连接成功/断开后的技能门控收口：domain 层按 show 增删技能落盘、show=true 时
/// 同步各 scope 禁用集 → 热刷 execpolicy 规则集（五轮评审 M-6：纯转发不刷
/// ruleset，在跑引擎 CLI 硬拦截过期 = fail-open）。声明式包仅回报在盘状态
/// （包内容不做门控写删）。
#[tauri::command]
pub async fn connector_apply_skills(
    id: String,
    pool: State<'_, EnginePool>,
) -> Result<Value, String> {
    let result = connectors_registry::apply_skills(&id).await?;
    pool.refresh_permission_rulesets().await;
    Ok(result)
}

/// 连接器开关：domain 层写停用标志并同步各 scope 禁用集 → 热刷 execpolicy 规则集
/// （四轮评审 M-6a：纯转发不刷 ruleset，在跑引擎 CLI 硬拦截过期 = fail-open）。
#[tauri::command]
pub async fn connector_set_enabled(
    id: String,
    enabled: bool,
    pool: State<'_, EnginePool>,
) -> Result<Value, String> {
    let result = connectors_registry::set_enabled(&id, enabled).await?;
    pool.refresh_permission_rulesets().await;
    Ok(result)
}

/// 给前端渲染开关态：`{connected, enabled, visible}`。
#[tauri::command]
pub async fn connector_skills_state(id: String) -> Result<Value, String> {
    connectors_registry::skills_state(&id).await
}

async_command_passthrough!(ima_domain, ima_status() -> Result<Value, String>);

/// ima 连接成功会安装配套技能 ima-skills（domain 层落盘）→ 重写在线会话组合目录
/// （skill 双 scope 治理事件驱动时机）+ 热刷 execpolicy 规则集（技能脚本 deny 规则
/// 随目录变化，四轮评审 M-6a）。失败时技能未装上，不重写。
#[tauri::command]
pub async fn ima_connect(
    client_id: String,
    api_key: String,
    pool: State<'_, EnginePool>,
) -> Result<Value, String> {
    let result = ima_domain::ima_connect(client_id, api_key).await?;
    pool.refresh_live_sessions_skills().await;
    pool.refresh_permission_rulesets().await;
    Ok(result)
}

/// ima 退出会卸载配套技能 ima-skills（domain 层落盘）→ 重写在线会话组合目录 +
/// 热刷 execpolicy 规则集（同上，M-6a）。
#[tauri::command]
pub async fn ima_logout(pool: State<'_, EnginePool>) -> Result<Value, String> {
    let result = ima_domain::ima_logout().await?;
    pool.refresh_live_sessions_skills().await;
    pool.refresh_permission_rulesets().await;
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
