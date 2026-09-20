// ---------------------------------------------------------------------------
// 工具市场
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_marketplace_tools()
-> Result<Vec<crate::features::marketplace::MarketplaceToolInfo>, String> {
    let mgr = crate::features::marketplace::MarketplaceManager::new();
    let tools = mgr.list_tools();
    Ok(tools)
}

/// 变更落盘后的统一热刷收尾（安装/卸载/更新/导入/恢复各命令共用，替代各自
/// 手写的 refresh 调用序列，保证顺序一致）：
/// - `disallowed`：mcp/spanner 供给面变化（导入/恢复出现新包）→ 先热刷引擎的
///   disallowed_tools 白名单；
/// - 随后重写在线会话组合目录（skills 影响两个 scope 的启用集，下一轮 prompt
///   生效）+ 刷新 deny 规则集（包的 CLI/技能脚本纳入/移出，M-6 热刷）。
async fn hot_refresh(
    pool: &tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
    disallowed: bool,
) {
    if disallowed {
        pool.refresh_disallowed_tools().await;
    }
    pool.refresh_live_sessions_skills().await;
    pool.refresh_permission_rulesets().await;
}

/// 上传展示名净化（仅写 bundles.json 的 upload 来源标记用）：去路径分隔符与
/// 控制字符，截 128 字符。zip 名与裸 `.md` 文件名两个上传通道共用同一口径。
fn sanitize_display_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(128)
        .collect()
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
    app: tauri::AppHandle,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    let user_config = config.unwrap_or_default();
    let install_tool_id = tool_id.clone();
    // DenyAll consent-gate registration BEFORE the install (deny-first,
    // #517 review round 4): a refused registration aborts before the package
    // is exposed or an existing installation is overwritten. The sync write
    // can block on the cross-process flock (#515): keep it off the executor.
    let gate_tool_id = tool_id.clone();
    tokio::task::spawn_blocking(move || install_marketplace_tool_gates(&gate_tool_id))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))??;

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

    // Companion skills, each deny-first registered then installed (deny-first
    // per companion, #517 review round 4: a refused registration skips that
    // companion instead of uninstalling a pre-existing copy). Off the
    // executor for the same flock reason as the gate above.
    let companion_tool_id = tool_id.clone();
    tokio::task::spawn_blocking(move || install_marketplace_tool_companions(&companion_tool_id))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))?;
    // 联动安装的 companion 技能影响两个 scope 的启用集：重写在线会话组合目录
    // （下一轮 prompt 即生效，与 uninstall_marketplace_tool 对称，skill 双 scope
    // 治理事件驱动时机 §2.3.2）。
    // mcp.json 可能新增了 server：递增修订号让在线引擎下一轮 get_or_spawn
    // 安全重建并重新发现工具（mark_mcp_config_updated 契约）。plain 会话的
    // 引擎读按会话派生的 mcp 配置（仅 spawn 时从全局 mcp.json 重写），不递增
    // 则中途安装的 server 对活跃引擎永不可见（PPT 场景实测：自动装完 pptx 后
    // 同会话 mcp_pptx_make_pptx 仍不可见）。code 会话读全局文件、底座每轮
    // mtime+hash 自愈，此时重建是冗余但无害的一次性开销（与 mark_model_updated
    // 同粒度的取舍）。
    pool.mark_mcp_config_updated();
    // Install state flows into the deny snapshot in both directions (the
    // NATIVE_PACKAGE_TOOLS ownership gate and the DenyAll scope syncs above
    // read it): without this refresh live engines keep the pre-install
    // admission — a package's native tool stays denied, and a newly installed
    // connector's tools stay admitted — until respawn.
    hot_refresh(&pool, true).await;
    crate::features::behavior_telemetry::track(
        &app,
        crate::features::behavior_telemetry::BehaviorEvent::new("tool_install_completed")
            .tool(&tool_id, &tool_id, "mcp")
            .success(true),
    );
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
pub async fn uninstall_marketplace_tool(
    tool_id: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    // The sync body includes scope-file RMW that can block on the
    // cross-process flock (#515): keep it off the executor.
    tokio::task::spawn_blocking(move || uninstall_marketplace_tool_sync(&tool_id))
        .await
        .map_err(|e| format!("uninstall join: {e}"))??;
    // mcp.json 可能移除了 server：递增修订号让在线引擎下一轮 get_or_spawn
    // 安全重建（同 install 路径，mark_mcp_config_updated 契约），残留的已卸
    // 连接器工具不再出现在模型目录。
    pool.mark_mcp_config_updated();
    // 联动卸载的 companion 技能影响两个 scope 的启用集：重写在线会话组合目录。
    // Keep the native-tool deny list uniform with the skill uninstall path
    // (no live effect today: no current MCP manifest owns a native tool, but
    // the uninstall postcondition must not depend on that accident).
    hot_refresh(&pool, true).await;
    Ok(())
}

pub(super) fn uninstall_marketplace_tool_sync(tool_id: &str) -> Result<(), String> {
    let mgr = crate::features::marketplace::MarketplaceManager::new();
    // Resolve companion ownership before any OAuth, skill, or MCP state is mutated.
    let companions = mgr.companion_skills(tool_id);
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
    // 联动:删配套技能。必须先于 `mgr.uninstall` 执行:技能落盘目录按
    // `skill_owner_package` 条件认领推导(包本体已装才归 `bundles/<pkg>/skills/`),
    // MCP 先卸则认领翻转、技能卸载会按「独立纯技能包」算错目录并报「非市场安装」
    // 静默残留(gongwen 先卸 → government-writing 删不掉的顺序依赖 bug)。
    // Companion teardown must also *succeed* before the MCP record is removed:
    // read-time scope normalization maps skill id -> package id one-way, so a
    // failed companion delete followed by MCP removal flips the claim back to
    // the skill name and the stored package-level disabled/hidden entries stop
    // matching — a user-disabled/hidden skill would be re-materialized into
    // sessions with its scripts outside the execpolicy deny rules. Abort on
    // failure (same discipline as the install path's abort-on-delete-failure):
    // the MCP stays installed, the claim stays stable, and the user can retry.
    //
    // Upload 组合包例外（B1）：companion 技能在 bundles.json 无独立登记，此时
    // MCP 未卸、`skill_owner_package` 仍判归本包 —— 走技能物理删除会把用户唯一
    // 副本的 `bundles/<pkg>/skills/<sid>` 删掉，随后整包回收只剩残缺包（kind
    // 退化为 mcp）。Upload 来源的包卸载 = 整包（含 skills/）进回收站，companion
    // 不单独物理删除，其 scope 清理挪到包卸载成功后（卸载失败则技能仍在，
    // 不应提前清 scope）。判定只认 bundles.json 登记的 Upload 来源；读失败按
    // 非 Upload 走原路径 —— 该路径的技能卸载自身对 bundles.json 读失败
    // fail-closed 中止（skill_marketplace::uninstall），不会误删。
    let recycles_with_package = crate::features::marketplace::store::BundleStore::new()
        .get(tool_id)
        .ok()
        .flatten()
        .is_some_and(|r| {
            matches!(
                r.source,
                crate::features::marketplace::store::BundleSource::Upload(_)
            )
        });
    for sid in &companions {
        if recycles_with_package {
            continue; // companion 随整包回收，见上注释
        }
        crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .uninstall(sid)
            .map_err(|e| format!("联动卸载配套技能 '{sid}' 失败（已中止工具卸载，请重试）: {e}"))?;
        // Scope entries are cleared only after the skill is actually gone —
        // otherwise a still-installed skill would be silently re-enabled. A
        // refused cleanup (lock unavailable, #515) only logs: the leftover
        // entry fails closed and the uninstall itself has already succeeded.
        if let Err(error) =
            crate::features::marketplace::scope::remove_bundle_from_disabled_scopes(sid)
        {
            eprintln!("[marketplace] scope cleanup for {sid} skipped: {error}");
        }
    }
    mgr.uninstall(tool_id)?;
    if recycles_with_package {
        // 整包已回收（companion 目录随包搬离）→ 此时技能确实没了，再清 scope。
        for sid in &companions {
            if let Err(error) =
                crate::features::marketplace::scope::remove_bundle_from_disabled_scopes(sid)
            {
                eprintln!("[marketplace] scope cleanup for {sid} skipped: {error}");
            }
        }
    }
    // 已卸载的连接器从两个 scope 的禁用集移除(避免残留 id)。
    // A refused cleanup (lock unavailable, #515) only logs: the leftover entry
    // fails closed and the uninstall itself has already succeeded.
    if let Err(error) = crate::features::marketplace::remove_bundle_from_disabled_scopes(tool_id)
    {
        eprintln!("[marketplace] scope cleanup for {tool_id} skipped: {error}");
    }
    Ok(())
}
// ---------------------------------------------------------------------------
// 技能市场（与工具市场并列：工具=MCP server，技能=SKILL.md 目录落 bundle/skills/）
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_marketplace_skills()
-> Result<Vec<crate::features::marketplace::skill_marketplace::MarketplaceSkillInfo>, String> {
    Ok(
        crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .list_skills(),
    )
}

#[tauri::command]
pub async fn install_marketplace_skill(
    skill_id: String,
    app: tauri::AppHandle,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    let install_skill_id = skill_id.clone();
    tokio::task::spawn_blocking(move || install_marketplace_skill_sync(&install_skill_id))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))??;
    // 安装影响两个 scope 的启用集：重写在线会话的组合目录（下一轮 prompt 生效）。
    // code scope 已初始化时新装技能默认仍关闭（sync 进 code 禁用集，见下面
    // install_marketplace_skill_sync），plain 会话立即可见。
    // The native-tool ownership gate (NATIVE_PACKAGE_TOOLS) reads install
    // state: without this refresh a package owning a native tool (ima) stays
    // denied in live engines until respawn.
    hot_refresh(&pool, true).await;
    crate::features::behavior_telemetry::track(
        &app,
        crate::features::behavior_telemetry::BehaviorEvent::new("tool_install_completed")
            .tool(&skill_id, &skill_id, "skill")
            .success(true),
    );
    Ok(())
}

/// Transaction boundary for every install/import path (review finding on
/// #517): the DenyAll consent-gate registration runs BEFORE any content lands
/// or replaces an existing installation (deny-first), so a refused
/// registration (lock unavailable / write failure / corrupt file) aborts the
/// operation with nothing touched — the package is never exposed outside the
/// deny lists of initialized DenyAll scopes, and a pre-existing installation
/// is never destroyed by a rollback (an uninstall-based rollback could not
/// restore an overwritten preset/upload copy anyway, review round 4).
fn refused_sync_error(what: &str, refused: String) -> String {
    format!("{what}: DenyAll sync refused before anything was installed: {refused}")
}

pub(super) fn install_marketplace_skill_sync(skill_id: &str) -> Result<(), String> {
    // 新装技能默认加入 DenyAll scope（当前 code）禁用集（与连接器同语义：
    // 外部能力显式开启）；组合目录由调用方在命令层重写（install_marketplace_skill）。
    // Deny-first (#517 review round 4): the registration only needs the
    // package id and must precede the install — a refusal aborts before
    // anything lands, so a re-install can never destroy the pre-existing
    // copy in a rollback.
    crate::features::marketplace::scope::sync_deny_all_scopes_after_install(skill_id)
        .map_err(|refused| refused_sync_error(&format!("skill '{skill_id}'"), refused))?;
    let mgr = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
    mgr.install(skill_id)
}

/// DenyAll consent-gate registration for `install_marketplace_tool`, run
/// BEFORE the package lands (deny-first, #517 review round 4): a refusal
/// aborts the install before the package is exposed or an existing
/// installation is overwritten — nothing on disk has been touched yet.
pub(super) fn install_marketplace_tool_gates(tool_id: &str) -> Result<(), String> {
    crate::features::marketplace::sync_deny_all_scopes_after_install(tool_id)
        .map_err(|refused| refused_sync_error(&format!("tool '{tool_id}'"), refused))
}

/// Companion skills for `install_marketplace_tool`, run AFTER the package
/// landed. Each companion is deny-first registered and then installed; a
/// refused registration or a failed install only logs and skips that
/// companion — a skill is an enhancement that must not fail the MCP install,
/// and skipping (instead of uninstalling) keeps a pre-existing companion copy
/// intact (review round 4). Infallible by contract: per-companion failures
/// are logged, never propagated.
pub(super) fn install_marketplace_tool_companions(tool_id: &str) {
    let mgr = crate::features::marketplace::MarketplaceManager::new();
    let skills = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
    // 联动:装该 MCP 声明的配套技能(引擎+引导整体到位)。
    for sid in mgr.companion_skills(tool_id) {
        // 新装的 companion 技能默认加入 DenyAll scope（当前 code）禁用集
        // （外部能力显式开启，与独立技能安装 install_marketplace_skill_sync 同语义）。
        if let Err(e) =
            crate::features::marketplace::scope::sync_deny_all_scopes_after_install(
                &sid,
            )
        {
            eprintln!("[marketplace] companion skill '{sid}' DenyAll sync refused, skipped: {e}");
            continue;
        }
        if let Err(e) = skills.install(&sid) {
            eprintln!("[marketplace] 配套技能 '{sid}' 安装失败: {e}");
        }
    }
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
    hot_refresh(&pool, false).await;
    Ok(())
}

/// 编辑上传包的 UI 展示名/说明（写 bundles.json 记录 extra 的
/// `display_name`/`display_description`，机读 id/目录/frontmatter name 不动）。
/// 仅 `source=Upload` 的记录可写；单技能包（`bundles/<id>/skills/` 下恰一个技能
/// 目录）的展示说明与 SKILL.md frontmatter description 双向同步（设覆盖回写
/// 新值并备份原值、清覆盖恢复原值）并重算内容指纹。门禁/校验/顺序契约都在
/// 特性层 `update_display_meta` 编排，命令层只做搬运与热刷收尾。
#[tauri::command]
pub async fn update_bundle_display_meta(
    id: String,
    display_name: Option<String>,
    display_description: Option<String>,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    // 展示说明在场时单技能包可能动 SKILL.md；展示名单独编辑（None）只写
    // extra，模型侧无感，不必热刷。
    let may_touch_skill_md = display_description.is_some();
    let result = tokio::task::spawn_blocking(move || {
        let mgr = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
        mgr.update_display_meta(&id, display_name.as_deref(), display_description.as_deref())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))
    .and_then(|r| r);
    // SKILL.md 回写/恢复改了包内容：热刷在线会话组合目录与 deny 规则集（与
    // update_marketplace_skill 同一收尾）。失败路径同样刷：sync 可能在报错前
    // 已动过 SKILL.md（窄窗口），失败点在 sync 前/中/后不可知，按「可能动过」
    // 处理——热刷是幂等的目录重扫，多刷一次无害。
    if may_touch_skill_md {
        hot_refresh(&pool, false).await;
    }
    result
}

/// 中文文件名 md 导入的无 frontmatter 兜底 id 派生。与 DefaultHasher 不同，
/// 不依赖进程内随机种子，重导/跨进程 id 一致。
fn stable_stem_hash(stem: &str) -> String {
    format!("{:016x}", crate::platform::paths::fnv1a64(stem.as_bytes()))
}

/// 把单个 `.md`/`.markdown` 技能文件的内容包装成「根放 SKILL.md 的裸 skill 包」走
/// 统一导入。frontmatter 有 `name` 用之；没有则用文件名 stem 兜底并注入最小
/// frontmatter。返回 PluginImportReport（调用方负责热刷 skills 组合目录）。
/// `pre_land` is forwarded as the pre-land hook of the unified import
/// pipeline (the DenyAll deny-first gate).
fn import_skill_md_content(
    md: String,
    filename: &str,
    pre_land: &dyn Fn(&str) -> Result<(), String>,
) -> Result<crate::features::marketplace::plugin_import::PluginImportReport, String> {
    use std::io::Write;
    let stem = filename
        .rfind('.')
        .map(|i| &filename[..i])
        .unwrap_or(filename);
    let fallback = crate::features::marketplace::skill_marketplace::sanitize_skill_name(stem);
    // 中文/纯符号文件名：sanitize 全映射为 `-` 后兜底恒为 "skill"，两个不同文件会
    // 静默互覆盖（二轮评审）。用文件名的稳定哈希派生唯一 id——同一文件重导 = 同 id
    // = 升级覆盖；不同文件 = 不同 id（FNV-1a 64 位，确定性、跨平台稳定）。
    let fallback = if fallback == "skill" && !stem.is_empty() {
        format!("skill-{}", stable_stem_hash(stem))
    } else {
        fallback
    };
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
    let display = sanitize_display_name(filename);
    let result = crate::features::marketplace::plugin_import::import_plugin_package_gated(
        &tmp.to_string_lossy(),
        &display,
        pre_land,
    );
    let _ = std::fs::remove_file(&tmp); // 清理临时文件(含失败路径)
    result
}

/// DenyAll consent gate for a freshly imported plugin package (zip and
/// wrapped-.md uploads share this; see `refused_sync_error` for the
/// transaction boundary). The closure form is the pre-land hook of the
/// unified import pipeline: the deny entry is registered before any content
/// lands, so a refusal aborts the import with nothing touched (#517 review
/// round 4 — an uninstall-based rollback could not restore an overwritten
/// pre-existing package anyway).
fn deny_all_pre_land(id: &str) -> Result<(), String> {
    crate::features::marketplace::sync_deny_all_scopes_after_install(id)
}

/// Import + DenyAll gate for one zip plugin package (the dialog and drag-drop
/// channels share this; callers only refresh pools on success).
pub(super) fn import_plugin_package_sync(
    zip_path: &str,
    display_name: &str,
) -> Result<crate::features::marketplace::plugin_import::PluginImportReport, String> {
    crate::features::marketplace::plugin_import::import_plugin_package_gated(
        zip_path,
        display_name,
        &deny_all_pre_land,
    )
}

/// Import + DenyAll gate for one wrapped-.md skill upload.
pub(super) fn import_skill_md_content_gated(
    md: String,
    filename: &str,
) -> Result<crate::features::marketplace::plugin_import::PluginImportReport, String> {
    import_skill_md_content(md, filename, &deny_all_pre_land)
}

/// 弹文件选择框选插件包并导入（plugin-protocol 统一上传：mcp/skill/组合包），
/// 或选单个 `.md`/`.markdown` 技能文件（包装成裸 skill 包）。返回 `Some(新包 id)`=
/// 已导入（前端据此打开展示信息编辑弹窗），`None`=用户取消。
///
/// 注：旧名 `import_spanner_package` 已重命名——脚本可执行能力并入 skill 包
/// 通过 SKILL.md frontmatter `tools[]` 段声明，不再有独立 spanner 组件。
#[tauri::command]
pub async fn import_plugin_package_cmd(
    app: tauri::AppHandle,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("技能/插件包 (zip, md)", &["zip", "md", "markdown"])
        .blocking_pick_file()
    else {
        return Ok(None); // 用户取消
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

    // Import + DenyAll consent gate in one blocking step (the gate write can
    // block on the cross-process flock, #515); a refused gate aborts the
    // import before anything lands (see `deny_all_pre_land`).
    let report = tokio::task::spawn_blocking(move || {
        if is_md {
            let md = std::fs::read_to_string(&path)
                .map_err(|e| format!("读技能文件失败（{}）: {e}", path.display()))?;
            import_skill_md_content_gated(md, &display)
        } else {
            import_plugin_package_sync(&path.to_string_lossy(), &display)
        }
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    // 新装包进入供给：mcp/spanner 热刷工具白名单 + skills 热刷会话组合目录。
    // 导入包含本地 MCP 时 mcp.json 已变，同样要递增修订号触发在线引擎下轮
    // 重建（与 install_marketplace_tool 同口径，mark_mcp_config_updated 契约）。
    pool.mark_mcp_config_updated();
    hot_refresh(&pool, true).await;
    log::info!(
        "[marketplace] 插件导入: id={} kind={:?} icon={}",
        report.id,
        report.kind,
        report.icon
    );
    // 返回新包 id：前端据此打开展示信息编辑弹窗（预填默认名，可直接保存）。
    Ok(Some(report.id))
}

/// 拖放导入插件包（统一上传，与 `import_plugin_package_cmd` 同语义）：前端把 zip 读成
/// base64 传这里，临时落盘后走 `import_plugin_package`。返回新包 id=已导入（前端
/// 据此打开展示信息编辑弹窗）。
///
/// 注：旧名 `import_spanner_package_bytes` 已重命名——见上面注释。
#[tauri::command]
pub async fn import_plugin_package_bytes_cmd(
    filename: String,
    data_base64: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<String, String> {
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
    let safe_name = sanitize_display_name(&filename);
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
    // Import + DenyAll consent gate in one blocking step (see
    // `import_plugin_package_sync`).
    let report = tokio::task::spawn_blocking(move || {
        import_plugin_package_sync(&tmp_for_import.to_string_lossy(), &safe_name)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?;
    let _ = std::fs::remove_file(&tmp); // 清理临时文件(含失败路径)
    let report = report?;
    // 新装包进入供给：mcp/spanner 热刷工具白名单 + skills 热刷会话组合目录。
    // 导入包含本地 MCP 时 mcp.json 已变，同样要递增修订号触发在线引擎下轮
    // 重建（与 install_marketplace_tool 同口径，mark_mcp_config_updated 契约）。
    pool.mark_mcp_config_updated();
    hot_refresh(&pool, true).await;
    Ok(report.id)
}

/// 拖放导入单个 `.md`/`.markdown` 技能文件：把裸 markdown 包装成「根放 SKILL.md 的
/// 裸 skill 包」走统一导入（复用裸技能回退识别 + 落盘 + 登记）。frontmatter 有
/// `name` 用之；没有则用文件名 stem 兜底并注入一个最小 frontmatter。返回新包 id=
/// 已导入（前端据此打开展示信息编辑弹窗）。
#[tauri::command]
pub async fn import_skill_md_bytes(
    filename: String,
    data_base64: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<String, String> {
    use base64::Engine as _;
    let lower = filename.to_ascii_lowercase();
    if !lower.ends_with(".md") && !lower.ends_with(".markdown") {
        return Err("仅支持 .md / .markdown 技能文件".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data_base64)
        .map_err(|e| format!("解码数据失败: {e}"))?;
    // 上传字节大小上限：与 zip 通道对齐，避免单条 .md 把磁盘写爆。
    use crate::features::marketplace::plugin_import::MAX_PLUGIN_SIZE_BYTES;
    if bytes.len() as u64 > MAX_PLUGIN_SIZE_BYTES {
        return Err(format!(
            "技能文件超过 {} MiB 上限",
            MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
        ));
    }
    let md = String::from_utf8(bytes).map_err(|e| format!("技能文件须为 UTF-8 文本: {e}"))?;
    let filename_for_import = filename.clone();
    // Import + DenyAll consent gate in one blocking step (see
    // `import_skill_md_content_gated`).
    let report = tokio::task::spawn_blocking(move || {
        import_skill_md_content_gated(md, &filename_for_import)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    pool.refresh_live_sessions_skills().await;
    // 导入包的 CLI/技能脚本纳入 deny 规则集（M-6：import 路径热刷）。
    pool.refresh_permission_rulesets().await;
    Ok(report.id)
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
    // The deny list is snapshot state in live engines, and uninstall (unlike
    // ima_logout) keeps the package's keyring credentials: a skill owning a
    // native tool (ima) would stay admitted and executable until respawn
    // without this refresh.
    hot_refresh(&pool, true).await;
    Ok(())
}

pub(super) fn uninstall_marketplace_skill_sync(skill_id: &str) -> Result<(), String> {
    crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
        .uninstall(skill_id)?;
    // 已卸载的技能从两个 scope 的禁用集移除（避免残留 id，与连接器同语义）。
    // A refused cleanup (lock unavailable, #515) only logs: the leftover entry
    // fails closed and the uninstall itself has already succeeded.
    if let Err(error) =
        crate::features::marketplace::scope::remove_bundle_from_disabled_scopes(skill_id)
    {
        eprintln!("[marketplace] scope cleanup for {skill_id} skipped: {error}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 插件中心回收站（Upload 来源包卸载的软删除：列表 / 恢复 / 彻底删除）
// ---------------------------------------------------------------------------

/// 回收站列表（只读；Web 端 access-policy 放行 list_*）。
#[tauri::command]
pub fn list_recycled_plugins()
-> Result<Vec<crate::features::marketplace::recycle_bin::RecycledPluginInfo>, String> {
    crate::features::marketplace::recycle_bin::RecycleBin::new().list()
}

/// 恢复回收站包为已安装状态（变更操作）：搬回 bundles/<id>/ + 重建登记 +
/// MCP 组件重新供给。返回 credentials_required 提示前端引导重填凭据。
#[tauri::command]
pub async fn restore_recycled_plugin(
    id: String,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<crate::features::marketplace::recycle_bin::RestoreRecycledResult, String> {
    let result = tokio::task::spawn_blocking(move || {
        crate::features::marketplace::recycle_bin::restore_plugin(&id)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    // mcp.json 可能重新写入了 server：递增修订号让在线引擎下一轮 get_or_spawn
    // 安全重建（与 install/import 同口径，mark_mcp_config_updated 契约），否则
    // 恢复的插件 server 对活跃 plain 引擎不可见。
    pool.mark_mcp_config_updated();
    // 恢复 = 重新进入供给：mcp 热刷工具白名单 + skills 热刷会话组合目录 +
    // 包脚本纳入 deny 规则集（与 import/uninstall 同一时机语义）。
    hot_refresh(&pool, true).await;
    Ok(result)
}

/// 彻底删除回收站包（变更操作，fail-closed：仅删清单中存在的条目）。
#[tauri::command]
pub async fn purge_recycled_plugin(id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::recycle_bin::RecycleBin::new().purge(&id)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 导出回收站包为 zip 插件包（变更操作；Web 只读不放行）：弹原生保存对话框，
/// zip 内容对齐插件包规范（可经统一导入管线重新导入）。
/// 返回 Some(保存路径)；用户取消 → None；失败 Err。
#[tauri::command]
pub async fn export_recycled_plugin(
    id: String,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    // 默认文件名：<包id>.zip（id 已满足 [a-z0-9-_] 安全字符集，无需净化；
    // 不用 display_name——裸 .md 上传的技能 display_name 是 "SKILL.md"，
    // 导出默认名会变成无意义的 "SKILL.md.zip"）。
    let default_name = recycle_export_default_name(&id);
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .add_filter("插件包 (zip)", &["zip"])
        .blocking_save_file()
    else {
        return Ok(None); // 用户取消保存对话框
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("解析文件路径: {e}"))?;
    let dest = path.to_string_lossy().into_owned();
    let export_id = id.clone();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::recycle_bin::RecycleBin::new()
            .export_package(&export_id, std::path::Path::new(&dest))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// 导出默认文件名：`<包id>.zip`（id 已是安全字符集，直接使用）。
fn recycle_export_default_name(id: &str) -> String {
    format!("{id}.zip")
}

/// 导出已安装包为标准插件包 zip（变更操作；Web 只读不放行）：弹原生保存
/// 对话框，zip 内容对齐插件包规范（可经统一导入管线重新导入；manifest args
/// 的包内绝对路径导出时还原为相对形式）。
/// 返回 Some(保存路径)；用户取消 → None；失败 Err。
#[tauri::command]
pub async fn export_installed_plugin(
    id: String,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let default_name = recycle_export_default_name(&id);
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .add_filter("插件包 (zip)", &["zip"])
        .blocking_save_file()
    else {
        return Ok(None); // 用户取消保存对话框
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("解析文件路径: {e}"))?;
    let dest = path.to_string_lossy().into_owned();
    let export_id = id.clone();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::package_export::export_installed_plugin(
            &export_id,
            std::path::Path::new(&dest),
        )
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    Ok(Some(path.to_string_lossy().into_owned()))
}

// ---------------------------------------------------------------------------
// 能力包就绪态（修复方案 V1：统一 bundle_readiness，收敛五个连接器 status 命令）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct BundleReadinessResult {
    pub installed: bool,
    pub ready: bool,
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
    bundle_readiness_with_store(bundle_id, SystemCredentialStore::new()).await
}

/// 凭据存储可注入的内层实现（与 ima.rs `status_with_store` 同风格）：命令入口注入
/// `SystemCredentialStore`，测试注入 `MemoryCredentialStore` —— 测试线程在任何平台都
/// 不触碰真实系统凭据仓库（current_thread runtime 的 `block_on` 会把 spawn_blocking
/// 任务泵回测试线程执行，真 keychain 在 macOS 会触发授权弹窗挂起）。
/// store 按值传入并 move 进 spawn_blocking，要求 `Send + 'static`。
async fn bundle_readiness_with_store<S>(
    bundle_id: String,
    store: S,
) -> Result<BundleReadinessResult, String>
where
    S: CredentialStore + Send + 'static,
{
    use crate::features::marketplace::bundle::{
        BundleKind, BundleRegistry, Readiness, keyring_target, readiness_for,
    };
    let reg = BundleRegistry::new();
    let Some(bundle) = reg.bundle(&bundle_id) else {
        return Err(format!("未知能力包 '{bundle_id}'"));
    };
    // CLI/ima 包的 installed 在注册表是保守占位（恒 false），此处用连接器 status
    // 的真实字段覆盖，避免对消费方产出 (installed=false, ready=true) 的矛盾组合。
    let (installed, ready, reason) = match bundle.kind {
        BundleKind::Cli => {
            let status = match bundle_id.as_str() {
                "feishu" => crate::features::connectors::feishu::feishu_status().await?,
                "wecom" => crate::features::connectors::wecom::wecom_status().await?,
                "dingtalk" => crate::features::connectors::dingtalk::dingtalk_status().await?,
                "tmeet" => crate::features::connectors::tmeet::tmeet_status().await?,
                other => return Err(format!("未知 CLI 包 '{other}'")),
            };
            let connected = connected_of(&status);
            // wecom/dingtalk/tmeet 返回 installed（CLI 二进制在位），
            // feishu 返回 configured（已配置）；都没有则退化为 connected。
            let installed = status
                .get("installed")
                .or_else(|| status.get("configured"))
                .and_then(|x| x.as_bool())
                .unwrap_or(connected);
            (
                installed,
                connected,
                if connected {
                    None
                } else {
                    Some("not_connected".to_string())
                },
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
            (creds || skill, ready, reason)
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
            (bundle.installed, ready, reason)
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
        installed,
        ready,
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

    /// Temp-home harness for the DenyAll transaction-boundary regressions:
    /// serializes on ENV_LOCK like the other PINVOU3_HOME mutators.
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-gate-rb-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Initialize the code scope while the cross-process lock still works
    /// (initialized is what makes the DenyAll sync a required write), then
    /// make the lock file unopenable (a directory at its path) so every
    /// consent-gate sync is refused.
    fn init_code_scope_then_break_lock() {
        crate::features::marketplace::save_disabled_bundles_for(
            crate::features::marketplace::ConnectorScope::Code,
            &["seed-bundle".to_string()],
        )
        .expect("code scope must initialize while the lock works");
        let lock = crate::platform::paths::pinvou3_home().join("disabled_bundles.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::create_dir_all(&lock).unwrap();
    }

    /// Transaction boundary (#517 review, deny-first): a preset skill install
    /// whose DenyAll consent-gate registration is refused must abort BEFORE
    /// the skill lands — it must not stay installed outside the deny lists of
    /// the initialized DenyAll scope, and nothing may be written at all. The
    /// refusal must name the lock failure (false-pass half of the #528
    /// pattern) and the persisted deny state must not gain the skill.
    #[test]
    fn install_skill_refused_before_landing_when_deny_sync_fails() {
        with_temp_home(|| {
            init_code_scope_then_break_lock();

            let error = install_marketplace_skill_sync("pptx").unwrap_err();
            assert!(
                error.contains("disabled_bundles.lock"),
                "refusal must name the lock failure: {error}"
            );
            assert!(
                error.contains("before anything was installed"),
                "refusal must state the deny-first boundary: {error}"
            );

            let skills =
                crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
            assert!(
                skills.find_skill_dir("pptx").is_none(),
                "the skill must never land on a refused gate"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("pptx")
                    .unwrap()
                    .is_none(),
                "no install record may be written on a refused gate"
            );
            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "the initialized deny state must be untouched"
            );
        });
    }

    /// Review round 4 regression, final semantics: re-installing an
    /// ALREADY-installed preset skill (the update path) must not touch the
    /// recorded consent state at all. The gate skips known bundles, so the
    /// reinstall goes through even with the scope lock unavailable and the
    /// skill stays ENABLED in the initialized DenyAll scope — the old
    /// unconditional re-registration silently disabled a working install,
    /// both on success and (with no recovery path) on any post-gate failure.
    #[test]
    fn reinstall_preset_skill_preserves_consent_state() {
        with_temp_home(|| {
            let skills =
                crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
            skills
                .install("pptx")
                .expect("first install must succeed while the lock works");
            let skill_md = skills
                .find_skill_dir("pptx")
                .expect("precondition: pptx installed")
                .join("SKILL.md");
            assert!(skill_md.is_file(), "precondition: SKILL.md on disk");

            init_code_scope_then_break_lock();

            install_marketplace_skill_sync("pptx")
                .expect("a known bundle's reinstall must not need the consent-gate write");
            assert!(
                skill_md.is_file(),
                "the reinstalled skill must still be on disk"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("pptx")
                    .unwrap()
                    .is_some(),
                "the install record must survive the reinstall"
            );
            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "the reinstall must not re-deny the enabled skill"
            );
        });
    }

    /// Same transaction boundary for the MCP tool path: the gate registration
    /// runs before the install, so a refusal must leave no package dir and no
    /// store record.
    #[test]
    fn install_tool_gates_refuse_before_anything_lands() {
        with_temp_home(|| {
            init_code_scope_then_break_lock();

            let error = install_marketplace_tool_gates("weather").unwrap_err();
            assert!(
                error.contains("disabled_bundles.lock"),
                "refusal must name the lock failure: {error}"
            );
            assert!(
                error.contains("before anything was installed"),
                "refusal must state the deny-first boundary: {error}"
            );
            assert!(
                !crate::platform::paths::bundles_root()
                    .join("weather")
                    .exists(),
                "the gate alone must not create the package dir"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("weather")
                    .unwrap()
                    .is_none(),
                "the gate alone must not write an install record"
            );
        });
    }

    /// Review round 4 regression, final semantics: an already-installed tool
    /// keeps its consent state across a reinstall — the gate skips known
    /// bundles, so even with the scope lock unavailable the gate returns
    /// `Ok` without writing and the previously-working installation stays
    /// ENABLED. The old unconditional re-registration would have re-denied
    /// it, and any post-gate install failure (pip, disk, remote validation)
    /// then left it disabled with no recovery path. The precondition install
    /// injects a `MemoryCredentialStore`: the default constructor would
    /// write AMAP_KEY to the REAL system keychain, which both fails on
    /// machines holding a weather credential (keyring refuses the
    /// conflicting write) and risks clobbering the user's real key.
    #[test]
    fn existing_tool_reinstall_preserves_consent_state() {
        with_temp_home(|| {
            let mgr = crate::features::marketplace::MarketplaceManager::with_store(
                crate::platform::credential_store::MemoryCredentialStore::default(),
            );
            let mut config = std::collections::HashMap::new();
            config.insert("AMAP_KEY".to_string(), "test-key".to_string());
            mgr.install("weather", &config)
                .expect("weather must install while the lock still works");
            let pkg_dir = crate::platform::paths::bundles_root().join("weather");
            assert!(pkg_dir.exists(), "precondition: weather landed on disk");

            init_code_scope_then_break_lock();

            install_marketplace_tool_gates("weather")
                .expect("a known bundle's gate must skip the consent-gate write");
            assert!(
                pkg_dir.exists(),
                "the pre-existing installation must be untouched by the gate"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("weather")
                    .unwrap()
                    .is_some(),
                "the pre-existing install record must survive"
            );
            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "the reinstall must not re-deny the enabled tool"
            );
        });
    }

    /// Review round 4 regression, final semantics: a companion that already
    /// exists as a standalone install is a known bundle for the consent
    /// gate, so its re-install never touches the deny state (the old
    /// unconditional registration re-denied it, and the pre-deny-first
    /// rollback destroyed the user's copy outright).
    #[test]
    fn existing_companion_reinstall_preserves_consent_state() {
        with_temp_home(|| {
            let skills =
                crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
            // government-writing is gongwen's declared companion; installed
            // standalone here it is a pure-skill package of its own.
            skills
                .install("government-writing")
                .expect("standalone companion install must succeed while the lock works");
            let skill_md = skills
                .find_skill_dir("government-writing")
                .expect("precondition: companion installed")
                .join("SKILL.md");

            init_code_scope_then_break_lock();

            // Companion handling is best-effort; with the gate skipping the
            // known bundle the companion re-installs normally.
            install_marketplace_tool_companions("gongwen");
            assert!(
                skill_md.is_file(),
                "the pre-existing companion must be untouched"
            );
            assert!(
                !crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                )
                .iter()
                .any(|id| id == "government-writing"),
                "the companion reinstall must not re-deny the enabled skill"
            );
        });
    }

    /// Final semantics for the unified plugin-package import channel: the
    /// consent gate runs before the same-content check but SKIPS installed
    /// bundles, so a rejected re-import hits the content-conflict refusal
    /// instead of silently flipping the package's consent state — the v1
    /// content and its enabled state stay intact.
    #[test]
    fn reimport_conflict_preserves_package_and_consent_state() {
        with_temp_home(|| {
            let plugin_json = r#"{"manifest_version":1,"id":"gate-rb-plugin","name":"p","components":{"skills":[{"id":"gate-rb-skill2","dir":"skills/gate-rb-skill2"}]}}"#;
            let zip_for = |skill_body: &str| {
                let mut zip_buf = std::io::Cursor::new(Vec::new());
                {
                    use std::io::Write;
                    let mut zw = zip::ZipWriter::new(&mut zip_buf);
                    let opts = zip::write::SimpleFileOptions::default();
                    zw.start_file("plugin.json", opts).unwrap();
                    zw.write_all(plugin_json.as_bytes()).unwrap();
                    zw.start_file("skills/gate-rb-skill2/SKILL.md", opts)
                        .unwrap();
                    zw.write_all(skill_body.as_bytes()).unwrap();
                    zw.finish().unwrap();
                }
                let tmp = std::env::temp_dir().join(format!(
                    "gate-rb-plugin-{}-{}.zip",
                    std::process::id(),
                    crate::platform::paths::tests::unique_suffix()
                ));
                std::fs::write(&tmp, zip_buf.into_inner()).unwrap();
                tmp
            };
            let v1 = zip_for("---\nname: gate-rb-skill2\ndescription: v1\n---\nbody v1");
            let v2 = zip_for("---\nname: gate-rb-skill2\ndescription: v2\n---\nbody v2");

            let report = import_plugin_package_sync(&v1.to_string_lossy(), "gate-rb-plugin.zip")
                .expect("first import must succeed while the lock works");
            assert_eq!(report.id, "gate-rb-plugin");

            init_code_scope_then_break_lock();

            let error = import_plugin_package_sync(&v2.to_string_lossy(), "gate-rb-plugin.zip")
                .unwrap_err();
            assert!(
                error.contains("已存在且内容不同"),
                "a rejected re-import must fail on the content conflict, not the gate \
                 (the gate skips installed bundles): {error}"
            );

            let pkg_dir = crate::platform::paths::bundles_root().join("gate-rb-plugin");
            assert!(
                pkg_dir.join("skills/gate-rb-skill2/SKILL.md").is_file(),
                "the pre-existing package must stay installed"
            );
            let content =
                std::fs::read_to_string(pkg_dir.join("skills/gate-rb-skill2/SKILL.md")).unwrap();
            assert!(
                content.contains("v1"),
                "the pre-existing v1 content must be intact: {content}"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("gate-rb-plugin")
                    .unwrap()
                    .is_some(),
                "the pre-existing install record must survive"
            );
            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "the rejected re-import must not flip the package's consent state"
            );
            let _ = std::fs::remove_file(&v1);
            let _ = std::fs::remove_file(&v2);
        });
    }

    /// 第九刀：bundle_readiness 响应携带完整 BundleInfo（前端功能事实数据源）。
    /// 凭据存在性经 `bundle_readiness_with_store` 注入 MemoryCredentialStore 现算，
    /// 不触碰真实系统凭据仓库（真 keychain 在 macOS 会触发授权弹窗挂起测试线程）。
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
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

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
        // 空内存凭据库：必填凭据缺失 → 未就绪（任何平台结果确定，不依赖本机 keychain 状态）
        let cred_store = crate::platform::credential_store::MemoryCredentialStore::default();
        let result = rt
            .block_on(bundle_readiness_with_store(
                "weather".to_string(),
                cred_store.clone(),
            ))
            .unwrap();

        assert!(result.installed, "store 有记录应已安装");
        assert!(!result.ready, "缺必填凭据应未就绪");
        assert!(result.actions.iter().any(|a| a.id == "configure"));
        let bundle = result.bundle.expect("响应应携带 BundleInfo");
        assert_eq!(bundle.version, "1.2.3");
        assert_eq!(bundle.description, "天气查询");
        assert_eq!(bundle.config_fields.len(), 1);
        assert_eq!(bundle.config_fields[0].key, "AMAP_KEY");
        assert!(bundle.config_fields[0].secret);

        // 反向断言：内存库写入必填凭据后应就绪——证明就绪判定确实消费注入的
        // store（而非恒定返回缺失）。target 缺省映射 env，与 tool_credentials 一致。
        cred_store
            .set(
                &crate::platform::credential_store::CredentialReference::for_mcp_secret(
                    "weather", "env", "AMAP_KEY",
                ),
                "test-amap-key",
            )
            .unwrap();
        let result = rt
            .block_on(bundle_readiness_with_store(
                "weather".to_string(),
                cred_store,
            ))
            .unwrap();
        assert!(result.ready, "必填凭据已注入内存库应就绪");

        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 导出默认文件名：恒为 `<包id>.zip`（id 安全字符集，无需净化；不再用
    /// display_name——裸 .md 上传的技能 display_name 是 "SKILL.md"，会导出
    /// 无意义的 "SKILL.md.zip"）。
    #[test]
    fn recycle_export_default_name_is_package_id_zip() {
        assert_eq!(recycle_export_default_name("my-pkg"), "my-pkg.zip");
        assert_eq!(recycle_export_default_name("up-skill"), "up-skill.zip");
    }

    /// B1 全链路回归：Upload 组合包（mcp/ + skills/）经真实命令路径
    /// `uninstall_marketplace_tool_sync` 卸载 —— companion 技能在 bundles.json 无
    /// 独立登记、MCP 未卸前 `skill_owner_package` 仍判归本包，此前被物理删除、
    /// 整包回收只剩残缺包（kind 退化 mcp）。修复后 companion 随整包回收：
    /// 回收条目 kind=bundle 且 skills/ 内容完整。
    /// 借 ENV_LOCK 与其它 mutate PINVOU3_HOME 的测试串行。
    #[test]
    fn uninstall_upload_bundle_via_command_recycles_companion_skills() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-uninstall-cmd-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        // 组合包落盘：mcp/manifest.json（声明 companion_skills）+ skills/<sid>/SKILL.md。
        let manifest_dir = crate::features::marketplace::mcp_catalog::package_mcp_dir("up-cmd");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(
            manifest_dir.join("manifest.json"),
            r#"{"id":"up-cmd","name":"UpCmd","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["up-cmd-skill"]}"#,
        )
        .unwrap();
        let skill_dir = crate::platform::paths::bundles_root().join("up-cmd/skills/up-cmd-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: up-cmd-skill\n---\n").unwrap();
        let mgr = crate::features::marketplace::MarketplaceManager::new();
        mgr.install_upload(
            "up-cmd",
            crate::features::marketplace::store::BundleSource::Upload("up-cmd.zip".to_string()),
        )
        .unwrap();
        assert!(
            skill_dir.join("SKILL.md").is_file(),
            "前置：companion 已落盘"
        );

        uninstall_marketplace_tool_sync("up-cmd").expect("命令路径卸载应成功");

        let recycled =
            crate::platform::paths::pinvou3_home().join("marketplace/recycle-bin/up-cmd");
        assert!(
            recycled.join("mcp/manifest.json").is_file(),
            "mcp/ 应完整保留在回收站"
        );
        assert!(
            recycled.join("skills/up-cmd-skill/SKILL.md").is_file(),
            "companion 技能应随整包回收，不得被物理删除"
        );
        assert!(
            !crate::platform::paths::bundles_root()
                .join("up-cmd")
                .exists(),
            "整包应搬离 bundles_root"
        );
        let list = crate::features::marketplace::recycle_bin::RecycleBin::new()
            .list()
            .unwrap();
        assert_eq!(list.len(), 1, "回收清单应有且仅有一条");
        assert_eq!(
            list[0].kind,
            crate::features::marketplace::recycle_bin::KIND_BUNDLE,
            "组合包回收条目 kind 应为 bundle（skills/ 未被先删）"
        );

        match prev {
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
