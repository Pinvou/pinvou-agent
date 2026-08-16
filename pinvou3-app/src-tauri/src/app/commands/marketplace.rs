// ---------------------------------------------------------------------------
// 工具市场
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_marketplace_tools(
) -> Result<Vec<crate::features::marketplace::MarketplaceToolInfo>, String> {
    let mgr = crate::features::marketplace::MarketplaceManager::new();
    let tools = mgr.list_tools();
    Ok(tools)
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceOAuthLoginResult {
    pub status: String,
    pub message: String,
    pub server_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceAuthStatus {
    pub installed: bool,
    pub mcp_configured: bool,
    pub oauth_required: bool,
    pub oauth_token_present: bool,
    pub status: String,
    pub server_name: Option<String>,
    pub message: String,
}

#[derive(Clone)]
struct ActiveMarketplaceOAuthLogin {
    request_id: String,
    cancellation_token: tokio_util::sync::CancellationToken,
    completion: tokio::sync::watch::Receiver<bool>,
}

#[derive(Default)]
pub(super) struct MarketplaceOAuthLoginCoordinator {
    state: tokio::sync::Mutex<MarketplaceOAuthLoginCoordinatorState>,
}

#[derive(Default)]
struct MarketplaceOAuthLoginCoordinatorState {
    active: std::collections::HashMap<String, ActiveMarketplaceOAuthLogin>,
    pending_cancellations: std::collections::HashMap<String, String>,
}

pub(super) struct MarketplaceOAuthLoginRegistration {
    pub(super) cancellation_token: tokio_util::sync::CancellationToken,
    pub(super) completion_sender: tokio::sync::watch::Sender<bool>,
    pub(super) previous_completion: Option<tokio::sync::watch::Receiver<bool>>,
}

impl MarketplaceOAuthLoginCoordinator {
    pub(super) async fn register(
        &self,
        tool_id: &str,
        request_id: &str,
    ) -> MarketplaceOAuthLoginRegistration {
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let (completion_sender, completion) = tokio::sync::watch::channel(false);
        let mut state = self.state.lock().await;
        let cancelled_before_register = state
            .pending_cancellations
            .remove(tool_id)
            .is_some_and(|pending_request_id| pending_request_id == request_id);
        let previous = state.active.insert(
            tool_id.to_string(),
            ActiveMarketplaceOAuthLogin {
                request_id: request_id.to_string(),
                cancellation_token: cancellation_token.clone(),
                completion,
            },
        );
        if let Some(previous) = previous.as_ref() {
            previous.cancellation_token.cancel();
        }
        if cancelled_before_register {
            cancellation_token.cancel();
        }
        MarketplaceOAuthLoginRegistration {
            cancellation_token,
            completion_sender,
            previous_completion: previous.map(|active| active.completion),
        }
    }

    pub(super) async fn is_current(&self, tool_id: &str, request_id: &str) -> bool {
        self.state
            .lock()
            .await
            .active
            .get(tool_id)
            .is_some_and(|active| active.request_id == request_id)
    }

    pub(super) async fn finish(
        &self,
        tool_id: &str,
        request_id: &str,
        completion_sender: tokio::sync::watch::Sender<bool>,
    ) {
        let mut state = self.state.lock().await;
        if state
            .active
            .get(tool_id)
            .is_some_and(|active| active.request_id == request_id)
        {
            state.active.remove(tool_id);
        }
        drop(state);
        let _ = completion_sender.send(true);
    }

    pub(super) async fn cancel(&self, tool_id: &str, request_id: &str) -> bool {
        let completion = {
            let mut state = self.state.lock().await;
            let Some(active) = state
                .active
                .get(tool_id)
                .filter(|active| active.request_id == request_id)
            else {
                if state.active.contains_key(tool_id) {
                    return false;
                }
                state
                    .pending_cancellations
                    .insert(tool_id.to_string(), request_id.to_string());
                return true;
            };
            active.cancellation_token.cancel();
            active.completion.clone()
        };
        wait_for_oauth_completion(completion).await;
        true
    }
}

pub(super) async fn wait_for_oauth_completion(mut completion: tokio::sync::watch::Receiver<bool>) {
    if *completion.borrow() {
        return;
    }
    let _ = completion.changed().await;
}

fn marketplace_oauth_login_coordinator() -> &'static MarketplaceOAuthLoginCoordinator {
    static COORDINATOR: std::sync::OnceLock<MarketplaceOAuthLoginCoordinator> =
        std::sync::OnceLock::new();
    COORDINATOR.get_or_init(MarketplaceOAuthLoginCoordinator::default)
}

#[tauri::command]
pub async fn install_marketplace_tool(
    tool_id: String,
    config: Option<std::collections::HashMap<String, String>>,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    let user_config = config.unwrap_or_default();
    let install_tool_id = tool_id.clone();
    tokio::task::spawn_blocking(move || {
        let mgr = crate::features::marketplace::MarketplaceManager::new();
        mgr.install(&install_tool_id, &user_config)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;

    let should_validate = {
        let mgr = crate::features::marketplace::MarketplaceManager::new();
        mgr.requires_remote_connection_validation(&tool_id)
    };
    if should_validate {
        let validation_result = {
            let mgr = crate::features::marketplace::MarketplaceManager::new();
            mgr.validate_remote_connection(&tool_id).await
        };
        if let Err(err) = validation_result {
            let rollback_tool_id = tool_id.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let mgr = crate::features::marketplace::MarketplaceManager::new();
                mgr.uninstall(&rollback_tool_id)
            })
            .await;
            return Err(err);
        }
    }

    tokio::task::spawn_blocking(move || {
        let mgr = crate::features::marketplace::MarketplaceManager::new();
        // 联动:装该 MCP 声明的配套技能(引擎+引导整体到位)。
        // skill 是增强,装失败只记日志、不让已成功的 MCP 安装回滚。
        for sid in mgr.companion_skills(&tool_id) {
            if let Err(e) =
                crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
                    .install(&sid)
            {
                eprintln!("[marketplace] 配套技能 '{sid}' 安装失败: {e}");
                continue;
            }
            // 新装的 companion 技能默认加入 DenyAll scope（当前 code）禁用集
            // （外部能力显式开启，与独立技能安装 install_marketplace_skill_sync 同语义）。
            crate::features::marketplace::skill_scope::sync_deny_all_scopes_after_skill_install(
                &sid,
            );
        }
        // DenyAll 模式的 scope(如 code)已初始化时,新装的连接器默认仍关闭(显式开启)。
        crate::features::marketplace::sync_deny_all_scopes_after_install(&tool_id);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    // 联动安装的 companion 技能影响两个 scope 的启用集：重写在线会话组合目录
    // （下一轮 prompt 即生效，与 uninstall_marketplace_tool 对称，skill 双 scope
    // 治理事件驱动时机 §2.3.2）。
    pool.refresh_live_sessions_skills().await;
    Ok(())
}

pub(super) fn marketplace_oauth_error_result(
    server_name: String,
    error: anyhow::Error,
) -> MarketplaceOAuthLoginResult {
    let detail = format!("{error:#}");
    let lower = detail.to_ascii_lowercase();
    let (status, message) = if lower.contains("oauth login was cancelled") {
        ("cancelled", "已取消等待浏览器授权，可稍后重新授权。")
    } else if lower.contains("timed out waiting for oauth callback") {
        (
            "timeout",
            "授权超时，未收到浏览器回调。请确认浏览器授权是否完成，关闭错误页后可重试。",
        )
    } else if lower.contains("service-error") || lower.contains("status code 404") {
        (
            "service_error",
            "OAuth 授权服务返回错误或 404，当前未完成授权。请稍后重试，或联系服务方确认该账号/应用权限。",
        )
    } else if lower.contains("oauth provider") || lower.contains("authorization") {
        (
            "provider_error",
            "OAuth 授权服务拒绝了本次授权，当前未完成连接。请确认账号权限后重试。",
        )
    } else {
        (
            "failed",
            "OAuth 授权失败，当前未完成连接。请重试；如仍失败，请保留浏览器错误页和日志。",
        )
    };

    eprintln!("[marketplace] MCP OAuth login for '{server_name}' failed: {detail}");
    MarketplaceOAuthLoginResult {
        status: status.to_string(),
        message: message.to_string(),
        server_name,
    }
}

fn marketplace_oauth_server_from_mcp_config(
    server_name: &str,
) -> Result<Option<deepseek_tui::mcp::McpServerConfig>, String> {
    let mcp_path = crate::platform::paths::mcp_config_path();
    if !mcp_path.is_file() {
        return Ok(None);
    }
    let content =
        std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json 失败: {e}"))?;
    let config: deepseek_tui::mcp::McpConfig =
        serde_json::from_str(&content).map_err(|e| format!("解析 mcp.json 失败: {e}"))?;
    Ok(config.servers.get(server_name).cloned())
}

pub(super) fn marketplace_auth_status_fields(
    installed: bool,
    oauth_required: bool,
    mcp_configured: bool,
    auth_status: Option<deepseek_tui::mcp::oauth::McpAuthStatus>,
) -> (&'static str, &'static str, bool) {
    if oauth_required
        && mcp_configured
        && matches!(
            auth_status,
            Some(deepseek_tui::mcp::oauth::McpAuthStatus::OAuth)
        )
    {
        (
            "connected",
            "OAuth 授权已完成，可以在新会话中使用该工具。",
            true,
        )
    } else if oauth_required && mcp_configured {
        (
            "config_installed_auth_pending",
            "已写入 MCP 配置，但尚未完成 OAuth 授权。",
            false,
        )
    } else if oauth_required && installed {
        (
            "auth_pending",
            "工具已安装，但 MCP 配置或授权状态不完整，请重新连接。",
            false,
        )
    } else if oauth_required {
        ("not_installed", "尚未连接该工具。", false)
    } else if installed {
        ("connected", "工具已安装。", false)
    } else {
        ("not_installed", "工具尚未安装。", false)
    }
}

#[tauri::command]
pub async fn get_marketplace_tool_auth_status(
    tool_id: String,
) -> Result<MarketplaceAuthStatus, String> {
    let mgr = crate::features::marketplace::MarketplaceManager::new();
    let installed = mgr.installed_ids().iter().any(|id| id == &tool_id);
    let server_name = mgr.oauth_remote_server_name(&tool_id);
    let oauth_required = server_name.is_some();
    let mut mcp_configured = false;
    let mut auth_status = None;

    if let Some(name) = server_name.as_deref() {
        match marketplace_oauth_server_from_mcp_config(name) {
            Ok(Some(server)) => {
                mcp_configured = true;
                auth_status =
                    Some(deepseek_tui::mcp::oauth::auth_status_for_server(name, &server).await);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "[marketplace] failed to read OAuth status for '{name}' from mcp.json: {error}"
                );
            }
        }
    }

    let (status, message, oauth_token_present) =
        marketplace_auth_status_fields(installed, oauth_required, mcp_configured, auth_status);

    Ok(MarketplaceAuthStatus {
        installed,
        mcp_configured,
        oauth_required,
        oauth_token_present,
        status: status.to_string(),
        server_name,
        message: message.to_string(),
    })
}

#[tauri::command]
pub async fn start_marketplace_tool_oauth_login(
    tool_id: String,
    request_id: String,
) -> Result<MarketplaceOAuthLoginResult, String> {
    let mgr = crate::features::marketplace::MarketplaceManager::new();
    let server_name = mgr
        .oauth_remote_server_name(&tool_id)
        .ok_or_else(|| format!("工具 '{tool_id}' 未声明远程 MCP OAuth 登录"))?;
    let mcp_path = crate::platform::paths::mcp_config_path();
    let content =
        std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json 失败: {e}"))?;
    let config: deepseek_tui::mcp::McpConfig =
        serde_json::from_str(&content).map_err(|e| format!("解析 mcp.json 失败: {e}"))?;
    let server = config
        .servers
        .get(&server_name)
        .cloned()
        .ok_or_else(|| format!("mcp.json 未找到服务 '{server_name}'"))?;

    let coordinator = marketplace_oauth_login_coordinator();
    let registration = coordinator.register(&tool_id, &request_id).await;
    if let Some(previous_completion) = registration.previous_completion {
        wait_for_oauth_completion(previous_completion).await;
    }
    if registration.cancellation_token.is_cancelled()
        || !coordinator.is_current(&tool_id, &request_id).await
    {
        coordinator
            .finish(&tool_id, &request_id, registration.completion_sender)
            .await;
        return Ok(MarketplaceOAuthLoginResult {
            status: "cancelled".to_string(),
            message: "已取消等待浏览器授权，可稍后重新授权。".to_string(),
            server_name,
        });
    }

    let login_result = deepseek_tui::mcp::oauth::perform_oauth_login_for_server_with_cancel(
        &server_name,
        &server,
        None,
        None,
        None,
        registration.cancellation_token.clone(),
    )
    .await;
    coordinator
        .finish(&tool_id, &request_id, registration.completion_sender)
        .await;

    match login_result {
        Ok(()) => Ok(MarketplaceOAuthLoginResult {
            status: "connected".to_string(),
            message: "OAuth 授权已完成。".to_string(),
            server_name,
        }),
        Err(e) => Ok(marketplace_oauth_error_result(server_name, e)),
    }
}

#[tauri::command]
pub async fn cancel_marketplace_tool_oauth_login(
    tool_id: String,
    request_id: String,
) -> Result<bool, String> {
    Ok(marketplace_oauth_login_coordinator()
        .cancel(&tool_id, &request_id)
        .await)
}

#[tauri::command]
pub fn uninstall_marketplace_tool(
    tool_id: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    uninstall_marketplace_tool_sync(&tool_id)?;
    // 联动卸载的 companion 技能影响两个 scope 的启用集：重写在线会话组合目录
    // （阻塞版：命令保持同步，目录体量小、重写极快）。
    pool.refresh_live_sessions_skills_blocking();
    Ok(())
}

pub(super) fn uninstall_marketplace_tool_sync(tool_id: &str) -> Result<(), String> {
    let mgr = crate::features::marketplace::MarketplaceManager::new();
    let companions = mgr.companion_skills(tool_id); // 卸前先取(manifest 不删,卸后也能读,保险先读)
    if let Some(server_name) = mgr.oauth_remote_server_name(tool_id) {
        match marketplace_oauth_server_from_mcp_config(&server_name)? {
            Some(server) => {
                deepseek_tui::mcp::oauth::delete_oauth_tokens_for_server(&server_name, &server)
                    .map_err(|e| format!("删除 MCP OAuth token 失败: {e:#}"))?;
            }
            None => {
                eprintln!(
                    "[marketplace] OAuth server '{server_name}' not found in mcp.json while uninstalling '{tool_id}'"
                );
            }
        }
    }
    mgr.uninstall(tool_id)?;
    // 已卸载的连接器从两个 scope 的禁用集移除(避免残留 id)。
    crate::features::marketplace::remove_connector_from_disabled_scopes(tool_id);
    // 联动:删配套技能(best-effort,删不掉不影响 MCP 卸载)。
    for sid in companions {
        let _ = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .uninstall(&sid);
        // 已卸载技能从两个 scope 禁用集清除残留。
        crate::features::marketplace::skill_scope::remove_skill_from_disabled_scopes(&sid);
    }
    Ok(())
}
// ---------------------------------------------------------------------------
// 技能市场（与工具市场并列：工具=MCP server，技能=SKILL.md 目录落 bundle/skills/）
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_marketplace_skills(
) -> Result<Vec<crate::features::marketplace::skill_marketplace::MarketplaceSkillInfo>, String> {
    Ok(
        crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .list_skills(),
    )
}

#[tauri::command]
pub async fn install_marketplace_skill(
    skill_id: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || install_marketplace_skill_sync(&skill_id))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))??;
    // 安装影响两个 scope 的启用集：重写在线会话的组合目录（下一轮 prompt 生效）。
    // code scope 已初始化时新装技能默认仍关闭（sync 进 code 禁用集，见下面
    // install_marketplace_skill_sync），plain 会话立即可见。
    pool.refresh_live_sessions_skills().await;
    Ok(())
}

pub(super) fn install_marketplace_skill_sync(skill_id: &str) -> Result<(), String> {
    crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
        .install(skill_id)?;
    // 新装技能默认加入 DenyAll scope（当前 code）禁用集（与连接器同语义：
    // 外部能力显式开启）；组合目录由调用方在命令层重写（install_marketplace_skill）。
    crate::features::marketplace::skill_scope::sync_deny_all_scopes_after_skill_install(skill_id);
    Ok(())
}

/// 更新已安装的预置技能:复用 `install` 的原子覆盖管线落最新嵌入资源。
/// 与"新装"的差异:不调 `sync_code_scope_after_skill_install`——更新保留
/// 用户现有的启用/停用状态,不把技能重新塞回 code 禁用集。
#[tauri::command]
pub async fn update_marketplace_skill(
    skill_id: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let mgr = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
        // 只接受"已安装的预置技能";未安装走 install,上传技能无嵌入新版可更。
        let installed = mgr
            .list_skills()
            .into_iter()
            .any(|s| s.id == skill_id && s.installed && !s.user_uploaded);
        if !installed {
            return Err(format!("技能 '{skill_id}' 非已安装预置技能,无法更新"));
        }
        mgr.install(&skill_id)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    // 内容变了:重写在线会话组合目录（下一轮 prompt 生效,与安装/卸载一致）。
    pool.refresh_live_sessions_skills().await;
    Ok(())
}

/// 弹文件选择框选 zip 技能包并导入。前端无法用 plugin-dialog 的 JS API
/// (单 HTML 无 bundler 引不进),所以选文件走 Rust 端 dialog。
/// 返回 true=已导入,false=用户取消。
#[tauri::command]
pub async fn import_skill_package(
    app: tauri::AppHandle,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("技能包 (zip)", &["zip"])
        .blocking_pick_file()
    else {
        return Ok(false); // 用户取消
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("解析文件路径: {e}"))?;
    tokio::task::spawn_blocking(move || {
        let mgr = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
        let name = mgr.import_package(&path.to_string_lossy())?;
        // 与商店安装同语义：上传技能默认加入 DenyAll scope（当前 code）禁用集
        // （外部能力显式开启）。
        crate::features::marketplace::skill_scope::sync_deny_all_scopes_after_skill_install(&name);
        Ok::<String, String>(name)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    // 重写在线会话组合目录（下一轮 prompt 生效）。
    pool.refresh_live_sessions_skills().await;
    Ok(true)
}

/// 把单个 `.md`/`.markdown` 技能文件的内容包装成「根放 SKILL.md 的裸 skill 包」走
/// 统一导入。frontmatter 有 `name` 用之；没有则用文件名 stem 兜底并注入最小
/// frontmatter。返回 PluginImportReport（调用方负责热刷 skills 组合目录）。
fn import_skill_md_content(
    md: String,
    filename: &str,
) -> Result<crate::features::marketplace::plugin_import::PluginImportReport, String> {
    use std::io::Write;
    let stem = filename
        .rfind('.')
        .map(|i| &filename[..i])
        .unwrap_or(filename);
    let fallback = crate::features::marketplace::skill_marketplace::sanitize_skill_name(stem);
    let mut md = md;
    if crate::features::marketplace::skill_marketplace::read_skill_name_from_str(&md).is_none() {
        md = format!("---\nname: {fallback}\n---\n\n{md}");
    }
    // 包装成临时 zip（根放 SKILL.md）走统一导入。
    let tmp = std::env::temp_dir().join(format!(
        "pinvou3-skillmd-{}-{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    {
        let f = std::fs::File::create(&tmp).map_err(|e| format!("写临时文件: {e}"))?;
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("SKILL.md", opts).map_err(|e| e.to_string())?;
        zw.write_all(md.as_bytes()).map_err(|e| e.to_string())?;
        zw.finish().map_err(|e| e.to_string())?;
    }
    // 展示名 = 原始文件名（写 bundles.json 的 upload 来源标记）。
    let display: String = filename
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(128)
        .collect();
    let result = crate::features::marketplace::plugin_import::import_plugin_package(
        &tmp.to_string_lossy(),
        &display,
    );
    let _ = std::fs::remove_file(&tmp); // 清理临时文件(含失败路径)
    result
}

/// 弹文件选择框选插件包并导入（plugin-protocol 统一上传：spanner/mcp/skill/组合包），
/// 或选单个 `.md`/`.markdown` 技能文件（包装成裸 skill 包）。返回 true=已导入，
/// false=用户取消。
#[tauri::command]
pub async fn import_spanner_package(
    app: tauri::AppHandle,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("技能/插件包 (zip, md)", &["zip", "md", "markdown"])
        .blocking_pick_file()
    else {
        return Ok(false); // 用户取消
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("解析文件路径: {e}"))?;
    let display = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plugin.zip".to_string());
    let is_md = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false);

    let report = tokio::task::spawn_blocking(move || {
        if is_md {
            let md = std::fs::read_to_string(&path)
                .map_err(|e| format!("读技能文件失败（{}）: {e}", path.display()))?;
            import_skill_md_content(md, &display)
        } else {
            crate::features::marketplace::plugin_import::import_plugin_package(
                &path.to_string_lossy(),
                &display,
            )
        }
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    // 新装包进入供给：mcp/spanner 热刷工具白名单 + skills 热刷会话组合目录。
    pool.refresh_disallowed_tools().await;
    pool.refresh_live_sessions_skills().await;
    log::info!(
        "[marketplace] 插件导入: id={} kind={:?} icon={}",
        report.id,
        report.kind,
        report.icon
    );
    Ok(true)
}

/// 拖放导入插件包（统一上传，与 `import_spanner_package` 同语义）：前端把 zip 读成
/// base64 传这里，临时落盘后走 `import_plugin_package`。返回 true=已导入。
#[tauri::command]
pub async fn import_spanner_package_bytes(
    filename: String,
    data_base64: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<bool, String> {
    use base64::Engine as _;
    if !filename.to_ascii_lowercase().ends_with(".zip") {
        return Err("仅支持 .zip 插件包".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data_base64)
        .map_err(|e| format!("解码 zip 数据失败: {e}"))?;
    let max_bytes = crate::features::marketplace::plugin_import::MAX_PLUGIN_SIZE_BYTES;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("插件包超过 {} MiB 上限", max_bytes / 1024 / 1024));
    }
    // 展示名净化(仅写 bundles.json 的 upload 来源标记用):去路径分隔符/控制字符,截 128
    let safe_name: String = filename
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(128)
        .collect();
    let tmp = std::env::temp_dir().join(format!(
        "pinvou3-plugin-{}-{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("写临时文件: {e}"))?;
    let tmp_for_import = tmp.clone();
    let report = tokio::task::spawn_blocking(move || {
        crate::features::marketplace::plugin_import::import_plugin_package(
            &tmp_for_import.to_string_lossy(),
            &safe_name,
        )
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?;
    let _ = std::fs::remove_file(&tmp); // 清理临时文件(含失败路径)
    report?;
    // 新装包进入供给：mcp/spanner 热刷工具白名单 + skills 热刷会话组合目录。
    pool.refresh_disallowed_tools().await;
    pool.refresh_live_sessions_skills().await;
    Ok(true)
}

/// 拖放导入单个 `.md`/`.markdown` 技能文件：把裸 markdown 包装成「根放 SKILL.md 的
/// 裸 skill 包」走统一导入（复用裸技能回退识别 + 落盘 + 登记）。frontmatter 有
/// `name` 用之；没有则用文件名 stem 兜底并注入一个最小 frontmatter。返回 true=已导入。
#[tauri::command]
pub async fn import_skill_md_bytes(
    filename: String,
    data_base64: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<bool, String> {
    use base64::Engine as _;
    let lower = filename.to_ascii_lowercase();
    if !lower.ends_with(".md") && !lower.ends_with(".markdown") {
        return Err("仅支持 .md / .markdown 技能文件".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data_base64)
        .map_err(|e| format!("解码数据失败: {e}"))?;
    let md = String::from_utf8(bytes).map_err(|e| format!("技能文件须为 UTF-8 文本: {e}"))?;
    let filename_for_import = filename.clone();
    tokio::task::spawn_blocking(move || import_skill_md_content(md, &filename_for_import))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))??;
    pool.refresh_live_sessions_skills().await;
    Ok(true)
}

/// 拖放导入:Windows WebView2 的 HTML5 文件拖放拿不到源文件路径
/// (`dragDropEnabled=false`,契约测试锁定,附件系统同走字节通道),所以前端把
/// zip 读成 base64 传这里,临时落盘后走 `import_package_named`。
/// 与 `import_skill_package`(原生文件对话框)返回语义一致:true=已导入。
#[tauri::command]
pub async fn import_skill_package_bytes(
    filename: String,
    data_base64: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<bool, String> {
    use base64::Engine as _;
    if !filename.to_ascii_lowercase().ends_with(".zip") {
        return Err("仅支持 .zip 技能包".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data_base64)
        .map_err(|e| format!("解码 zip 数据失败: {e}"))?;
    use crate::features::marketplace::skill_marketplace::MAX_SKILL_SIZE_BYTES;
    if bytes.len() as u64 > MAX_SKILL_SIZE_BYTES {
        return Err(format!(
            "技能包超过 {} MiB 上限",
            MAX_SKILL_SIZE_BYTES / 1024 / 1024
        ));
    }
    // 展示名净化(仅写 .installed-from 标记用):去路径分隔符/控制字符,截 128
    let safe_name: String = filename
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(128)
        .collect();
    let tmp = std::env::temp_dir().join(format!(
        "pinvou3-skill-{}-{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("写临时文件: {e}"))?;
    let tmp_for_import = tmp.clone();
    let name = tokio::task::spawn_blocking(move || {
        let mgr = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
        let name = mgr.import_package_named(&tmp_for_import.to_string_lossy(), &safe_name)?;
        // 与商店安装同语义：上传技能默认加入 DenyAll scope（当前 code）禁用集
        // （外部能力显式开启）。
        crate::features::marketplace::skill_scope::sync_deny_all_scopes_after_skill_install(&name);
        Ok::<String, String>(name)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?;
    let _ = std::fs::remove_file(&tmp); // 清理临时文件(含失败路径)
    name?;
    // 与对话框导入一致:重写在线会话组合目录(下一轮 prompt 生效)。
    pool.refresh_live_sessions_skills().await;
    Ok(true)
}

#[tauri::command]
pub async fn uninstall_marketplace_skill(
    skill_id: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || uninstall_marketplace_skill_sync(&skill_id))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))??;
    // 卸载影响两个 scope 的启用集：重写在线会话的组合目录。
    pool.refresh_live_sessions_skills().await;
    Ok(())
}

pub(super) fn uninstall_marketplace_skill_sync(skill_id: &str) -> Result<(), String> {
    crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
        .uninstall(skill_id)?;
    // 已卸载的技能从两个 scope 的禁用集移除（避免残留 id，与连接器同语义）。
    crate::features::marketplace::skill_scope::remove_skill_from_disabled_scopes(skill_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// 能力包就绪态（修复方案 V1：统一 bundle_readiness，收敛五个连接器 status 命令）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct BundleReadinessResult {
    pub bundle_id: String,
    pub installed: bool,
    pub ready: bool,
    pub reason: Option<String>,
    /// 原连接器 status 的完整 detail（CLI/ima 型透传，向前兼容）
    pub detail: Option<serde_json::Value>,
    /// 动作下发（§3.3）：后端按当前状态推导的可用动作集。serde default 保持
    /// 契约纯增量；前端切换为动作渲染器在后续 PR。
    #[serde(default)]
    pub actions: Vec<crate::features::marketplace::actions::BundleAction>,
    /// 包功能事实全量（§3.1：description/version/category/config_fields 等，
    /// 第九刀增补）——前端详情三栏与配置弹窗的后端数据源；serde default 保持
    /// 契约纯增量。
    #[serde(default)]
    pub bundle: Option<crate::features::marketplace::bundle::BundleInfo>,
}

/// 统一就绪态查询：
/// - CLI 包（feishu/wecom/dingtalk/tmeet）→ 分派到各连接器 status，connected 即 ready；
///   installed 取 status 的 installed/configured 真实字段
/// - ima（凭据型技能包）→ ima_status：ready = connected（凭据齐且 companion 技能已装），
///   installed = 凭据或技能任一已配置
/// - MCP/技能/上传包 → 注册表 `readiness_for`（credentials 必填项查系统凭据现算）
#[tauri::command]
pub async fn bundle_readiness(bundle_id: String) -> Result<BundleReadinessResult, String> {
    use crate::features::marketplace::bundle::{
        keyring_target, readiness_for, BundleKind, BundleRegistry, Readiness,
    };
    let reg = BundleRegistry::new();
    let Some(bundle) = reg.bundle(&bundle_id) else {
        return Err(format!("未知能力包 '{bundle_id}'"));
    };
    // CLI/ima 包的 installed 在注册表是保守占位（恒 false），此处用连接器 status
    // 的真实字段覆盖，避免对消费方产出 (installed=false, ready=true) 的矛盾组合。
    let (installed, ready, reason, detail) = match bundle.kind {
        BundleKind::Cli => {
            let (connected, detail) = match bundle_id.as_str() {
                "feishu" => {
                    let v = crate::features::connectors::feishu::feishu_status().await?;
                    (connected_of(&v), Some(v))
                }
                "wecom" => {
                    let v = crate::features::connectors::wecom::wecom_status().await?;
                    (connected_of(&v), Some(v))
                }
                "dingtalk" => {
                    let v = crate::features::connectors::dingtalk::dingtalk_status().await?;
                    (connected_of(&v), Some(v))
                }
                "tmeet" => {
                    let v = crate::features::connectors::tmeet::tmeet_status().await?;
                    (connected_of(&v), Some(v))
                }
                other => return Err(format!("未知 CLI 包 '{other}'")),
            };
            // wecom/dingtalk/tmeet 返回 installed（CLI 二进制在位），
            // feishu 返回 configured（已配置）；都没有则退化为 connected。
            let installed = detail
                .as_ref()
                .and_then(|v| {
                    v.get("installed")
                        .or_else(|| v.get("configured"))
                        .and_then(|x| x.as_bool())
                })
                .unwrap_or(connected);
            (
                installed,
                connected,
                if connected {
                    None
                } else {
                    Some("not_connected".to_string())
                },
                detail,
            )
        }
        BundleKind::Skill if bundle.id == "ima" => {
            let v = crate::features::connectors::ima::ima_status().await?;
            let creds = v
                .get("credentials_present")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let skill = v
                .get("skill_installed")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            // ready 与 ima_status.connected 同义：凭据齐且 companion 技能已装
            let ready = creds && skill;
            let reason = if ready {
                None
            } else if !creds {
                Some("missing_credentials".to_string())
            } else {
                Some("skill_not_installed".to_string())
            };
            (creds || skill, ready, reason, Some(v))
        }
        _ => {
            // keychain 读可能阻塞数秒甚至数分钟（macOS 首次访问弹授权窗），
            // 与 ima/install 命令一致移出 async 线程：spawn_blocking 里按声明序
            // 预查必填凭据（同 key 多 target 取首个声明，保持原 find-first 语义），
            // has 闭包只读内存结果。
            let mut specs: Vec<(String, &'static str)> = Vec::new();
            for c in &bundle.credentials {
                if c.required && !specs.iter().any(|(k, _)| k == &c.key) {
                    specs.push((c.key.clone(), keyring_target(c.target)));
                }
            }
            let id = bundle_id.clone();
            let present: std::collections::HashSet<String> =
                tokio::task::spawn_blocking(move || {
                    let store = crate::platform::credential_store::SystemCredentialStore::new();
                    specs
                        .into_iter()
                        .filter(|(key, target)| {
                            store
                                .get(
                                    &crate::platform::credential_store::CredentialReference::for_mcp_secret(
                                        &id, target, key,
                                    ),
                                )
                                .ok()
                                .flatten()
                                .is_some()
                        })
                        .map(|(key, _)| key)
                        .collect()
                })
                .await
                .map_err(|e| format!("spawn_blocking: {e}"))?;
            let has = |key: &str| present.contains(key);
            let (ready, reason) = match readiness_for(&bundle, has) {
                Readiness::Ready => (true, None),
                Readiness::NotReady(reason) => (false, Some(reason.to_string())),
            };
            (bundle.installed, ready, reason, None)
        }
    };
    // 动作推导输入的 Readiness 重建：CLI/ima 分支的 reason 是自定义字符串，
    // 推导只区分 Ready / missing_credentials / 其它（见 actions.rs 规则注释）。
    let readiness = if ready {
        Readiness::Ready
    } else if reason.as_deref() == Some("missing_credentials") {
        Readiness::NotReady("missing_credentials")
    } else {
        Readiness::NotReady("not_ready")
    };
    let actions = crate::features::marketplace::actions::actions_for(&bundle, readiness);
    Ok(BundleReadinessResult {
        bundle_id,
        installed,
        ready,
        reason,
        detail,
        actions,
        bundle: Some(bundle),
    })
}

fn connected_of(v: &serde_json::Value) -> bool {
    v.get("connected")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
}
use super::prelude::*;

/// 导出《插件包设计规范》Markdown：打开系统保存对话框写入规范文档，方便用户
/// 直接下载、分发给第三方包作者。规范单一真相源在 `docs/plugin-package-spec.md`
/// （编译期内嵌，离线可用，不与运行时磁盘状态耦合）。
#[tauri::command]
pub async fn export_plugin_spec(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    const SPEC_MD: &str = include_str!("../../../../../docs/plugin-package-spec.md");
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name("pinvou-plugin-package-spec.md")
        .add_filter("Markdown", &["md"])
        .blocking_save_file()
    else {
        return Ok(false); // 用户取消保存对话框
    };
    let path = picked
        .into_path()
        .map_err(|error| format!("resolve_spec_export_path: {error}"))?;
    tokio::task::spawn_blocking(move || std::fs::write(&path, SPEC_MD))
        .await
        .map_err(|error| format!("spec_export_task_failed: {error}"))?
        .map_err(|error| format!("spec_export_write_failed: {error}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 第九刀：bundle_readiness 响应携带完整 BundleInfo（前端功能事实数据源）。
    #[test]
    fn bundle_readiness_carries_bundle_facts() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-readiness-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PINVOU3_HOME", &dir);

        // 带必填凭据的 MCP manifest + BundleStore 安装记录
        let manifest_dir = crate::features::marketplace::mcp_catalog::package_mcp_dir("weather");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(
            manifest_dir.join("manifest.json"),
            r#"{"id":"weather","name":"高德天气","description":"天气查询","version":"1.2.3","icon":"","category":"life","mcp_tools":[],"command":"","args":[],"config_fields":[{"key":"AMAP_KEY","label":"k","required":true,"secret":true}]}"#,
        )
        .unwrap();
        let store = crate::features::marketplace::store::BundleStore::new();
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "weather",
                    crate::features::marketplace::store::BundleSource::Preset,
                ),
            )
            .unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt
            .block_on(bundle_readiness("weather".to_string()))
            .unwrap();

        assert!(result.installed, "store 有记录应已安装");
        assert!(!result.ready, "缺必填凭据应未就绪");
        assert!(result.actions.iter().any(|a| a.id == "configure"));
        let bundle = result.bundle.expect("响应应携带 BundleInfo");
        assert_eq!(bundle.version, "1.2.3");
        assert_eq!(bundle.description, "天气查询");
        assert_eq!(bundle.category, "life");
        assert_eq!(bundle.config_fields.len(), 1);
        assert_eq!(bundle.config_fields[0].key, "AMAP_KEY");
        assert!(bundle.config_fields[0].secret);

        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
