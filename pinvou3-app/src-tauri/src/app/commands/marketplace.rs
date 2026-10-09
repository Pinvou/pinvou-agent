// ---------------------------------------------------------------------------
// 工具市场
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_marketplace_tools()
-> Result<Vec<crate::features::marketplace::MarketplaceToolInfo>, String> {
    let mgr = crate::features::marketplace::MarketplaceManager::new();
    let mut tools = mgr.list_tools();
    // bundle_version for builtin plugins is filled at the command layer (the
    // bundle version the app ships with): a marketplace -> runtime_bundle
    // dependency would be a feature cycle (architecture guard
    // rust_cyclic_feature_dependencies baseline is 0); app -> features is fine.
    for tool in &mut tools {
        if tool.builtin {
            tool.bundle_version =
                Some(crate::features::runtime_bundle::platform::BUNDLE_VERSION.to_string());
        }
    }
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

/// The deny-first core of `install_marketplace_tool`, as one ordered unit:
/// consent-gate registration BEFORE the install (deny-first, #517 review
/// round 4), so a refused gate leaves nothing landed and a post-gate
/// failure leaves only a harmless (fail-closed) deny entry for a
/// not-installed id. Sync and free of tauri types so the deny-first wiring
/// itself is directly testable —
/// `install_tool_sync_refused_before_anything_lands` drives this function
/// end-to-end, not the extracted gate helper in isolation (that one is
/// pinned by `install_tool_gates_refuse_before_anything_lands`).
fn install_marketplace_tool_sync(
    tool_id: &str,
    user_config: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    // Round-16 (review): probe catalog existence BEFORE the deny-first gate —
    // a garbage direct-IPC id would otherwise seed phantom deny rows, default-
    // off markers and ledger entries for a package that cannot exist (pure
    // over-denial surviving until the next composer full-list write). Same
    // lookup and same error as the install's own validation, so the UX for a
    // bad id is unchanged apart from timing.
    match crate::features::marketplace::mcp_catalog::embedded_manifest(tool_id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(format!("tool '{tool_id}' does not exist")),
        Err(error) => return Err(error),
    }
    install_marketplace_tool_gates(tool_id)?;
    crate::features::marketplace::MarketplaceManager::new().install(tool_id, user_config)
}

#[tauri::command]
pub async fn install_marketplace_tool(
    tool_id: String,
    config: Option<std::collections::HashMap<String, String>>,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    let user_config = config.unwrap_or_default();
    let install_tool_id = tool_id.clone();
    // The sync write can block on the cross-process flock (#515): keep the
    // deny-first core off the executor.
    tokio::task::spawn_blocking(move || {
        install_marketplace_tool_sync(&install_tool_id, &user_config)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;

    install_marketplace_tool_post_install(tool_id).await?;
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
    Ok(())
}

/// Post-install legs of `install_marketplace_tool`, extracted verbatim so the
/// consent-sync / validation ordering and its rollback semantics are
/// regression-testable without a Tauri harness (review #455 round-23 MAJOR 1c:
/// the behavior shipped in merge `de04d54a9` had zero test pins). Order is
/// load-bearing: the consent sync runs BEFORE the network validation; the
/// rollback uninstall's teardown removes the rows the sync wrote.
pub(super) async fn install_marketplace_tool_post_install(tool_id: String) -> Result<(), String> {
    // Round-21 MAJOR 2: the consent sync must run IMMEDIATELY after the
    // install commit, BEFORE the network validation — for initialized scopes
    // the stored list is the consent store, and a crash during the network
    // round-trip below would otherwise leave the pack ON in every initialized
    // scope with zero consent, with nothing at boot to reconcile it. The
    // crash window is now the ms-wide span between two adjacent fs-backed
    // operations. Ordering is safe against validation failure: the rollback
    // uninstall's teardown removes the entries this sync wrote (the exact
    // form, `remove_bundle_from_disabled_scopes_exact`). That teardown
    // degrades persist
    // failures to `log::warn`, so a rollback-time persist failure can still
    // strand a consent row for a pack that is gone — stale-deny, i.e. the
    // fail-closed direction (review #455 round-22 minor 2).
    let consent_tool_id = tool_id.clone();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::sync_deny_all_scopes_after_install(&consent_tool_id)
    })
    .await
    .map_err(|e| {
        // Round-24 minor 6: a join failure leaves the same terminal state as
        // a persist failure — the pack stays installed with zero consent
        // rows — so the honest sibling wording applies here too (skill path
        // :640-645; review #455 round-22 MAJOR 1). No rollback runs on this
        // arm.
        format!(
            "connector '{tool_id}' installed, but its consent state could not be applied (background task failed): new sessions will enable it by default — turn it off in the tools list: {e}"
        )
    })?
    .map_err(|e| {
        // Honest sibling wording (skill path :640-645): no rollback runs on
        // this arm — the pack stays installed with zero consent rows, so the
        // message must say exactly that (review #455 round-22 MAJOR 1).
        format!(
            "connector '{tool_id}' installed, but {}: new sessions will enable it by default — turn it off in the tools list: {e}",
            crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
        )
    })?;

    let should_validate = {
        let mgr = crate::features::marketplace::MarketplaceManager::new();
        mgr.requires_remote_connection_validation(&tool_id)
    };
    if should_validate {
        // Only consumes the validation outcome: on failure, roll back the installed tool and surface a user-readable error.
        if let Err(err) = {
            let mgr = crate::features::marketplace::MarketplaceManager::new();
            mgr.validate_remote_connection(&tool_id).await
        } {
            let rollback_tool_id = tool_id.clone();
            let rollback_result = tokio::task::spawn_blocking(move || {
                let mgr = crate::features::marketplace::MarketplaceManager::new();
                let result = mgr.uninstall(&rollback_tool_id);
                // The consent rows were written by THIS install attempt's own
                // deny-first gate milliseconds ago, so a SUCCEEDING rollback
                // uninstall strips them even when it is vacuous at the record
                // level (the manager's vacuous gate is a user-uninstall
                // policy; the rollback owns these rows). On a FAILED
                // uninstall the package stays installed — the rows must stay
                // too (fail-closed: installed + denied beats installed +
                // enabled), and the primary error surfaces for a retry.
                // Round-16 disclosure (review): this strip runs after the
                // uninstall returns, outside the transaction and import locks
                // — the same round-12 B2 window shape the skill lane carries
                // (see capability-governance §3.2): a concurrent same-id
                // deny-first registration landing in the window can be wiped
                // by this strip. Owned-rows-only by construction. Direction:
                // fail-closed while the wiped registration's pack has NOT
                // landed (the next gate run re-registers an unknown id), but
                // fail-open once that pack's record and content exist — the
                // known-bundle skip then suppresses every later gate run, so
                // the wiped rows are not restored until a teardown
                // (round-17 review; the strip is kept for the vacuous-rollback
                // case, where it removes this install attempt's own rows).
                if result.is_ok() {
                    if let Err(e) = crate::features::marketplace::scope::
                        remove_bundle_from_disabled_scopes_exact(&rollback_tool_id)
                    {
                        log::warn!(
                            "[marketplace] rollback consent-row strip for {rollback_tool_id} failed (stale-deny residue): {e}"
                        );
                    }
                }
                result
            })
            .await;
            // Best-effort compensation: LOG a rollback failure instead of
            // discarding it — the validation error remains the one returned
            // (round-28 nit: the rollback result is not surfaced to the user;
            // on a failed rollback the pack remains installed — fail-closed
            // state, the log line is the only trace).
            match &rollback_result {
                Err(e) => {
                    log::warn!(
                        "[marketplace] rollback uninstall join failed after validation error: {e}"
                    )
                }
                Ok(Err(e)) => {
                    log::warn!(
                        "[marketplace] rollback uninstall failed after validation error: {e}"
                    )
                }
                Ok(Ok(_)) => {}
            }
            return Err(err);
        }
    }

    tokio::task::spawn_blocking(move || {
        let mgr = crate::features::marketplace::MarketplaceManager::new();
        // 联动:装该 MCP 声明的配套技能(引擎+引导整体到位)。
        // skill 是增强,装失败只记日志、不让已成功的 MCP 安装回滚。
        // The tool's own consent sync already ran right after the install
        // commit (round-21 MAJOR 2, before the network validation); only the
        // companion loop remains here.
        let normalized_tool_id = crate::features::marketplace::scope::to_package_id(&tool_id);
        for sid in mgr.companion_skills(&tool_id) {
            // Round-16 (review): on the known-pack-shield edge the companion
            // normalizes to a DIFFERENT pack — its consent write is then a
            // real registration for that pack and must run deny-first,
            // BEFORE the install lands content, so a refused registration
            // aborts with nothing touched (the post-landing sync below stays
            // as the fail-visible belt; the known-clause makes it skip once
            // the pre-land registration landed). The common case normalizes
            // to the tool's own id, which the tool-level sync above already
            // registered.
            let divergent =
                crate::features::marketplace::scope::to_package_id(&sid) != normalized_tool_id;
            if divergent {
                crate::features::marketplace::scope::sync_deny_all_scopes_after_install(&sid)
                    .map_err(|refused| {
                        // Not the shared "before anything was installed" copy:
                        // this gate runs inside the post-install phase, so the
                        // parent tool's record and content have already
                        // committed — word it like the post-landing arms
                        // instead (round-19 review).
                        format!(
                            "tool '{tool_id}' installed, but companion skill '{sid}' was not: DenyAll sync refused: {refused}"
                        )
                    })?;
            }
            if let Err(e) =
                crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
                    .install(&sid)
            {
                eprintln!("[marketplace] 配套技能 '{sid}' 安装失败: {e}");
                continue;
            }
            if !divergent {
                continue;
            }
            // A newly installed companion skill joins the DenyAll scope disabled
            // sets by default (external capabilities are explicit opt-in, same
            // semantics as the standalone install_marketplace_skill_sync).
            // Round-33 MAJOR 1 (review #455): the sync is PROVABLY redundant —
            // the arm the old comment's "must not fail the whole command"
            // stance relied on — only when the companion normalizes to the
            // tool's own package id (the tool-level sync above stored exactly
            // that id). On the known-pack-shield edge (a standalone pack dir
            // named like the companion) the normalized id differs and the
            // sync is a real consent write for a different pack; a lost write
            // there leaves the pack live with zero consent in initialized
            // scopes with no boot reconciliation (the ledger-gated startup
            // refresh covers only the four CLI connector gates). That edge
            // therefore fails the command, exactly like the tool-level
            // sync's fail-visible persist above.
            if let Err(e) = crate::features::marketplace::scope::sync_deny_all_scopes_after_install(&sid)
            {
                return Err(format!(
                    "companion skill '{sid}' installed, but {}: new sessions will enable it by default — turn it off in the tools list: {e}",
                    crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
                ));
            }
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
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
    // Round-24 MAJOR 5: this read was a bare `read_to_string` on the Tokio
    // worker — a planted FIFO at ~/.pinvou3/mcp.json blocks `open()` forever
    // and pins the executor thread (the dialog is UI-retryable, so the pool
    // can be exhausted). Route it through the hardened private-data read
    // (refuses non-regular files fail-loud) and off the executor like every
    // other blocking call in this file.
    let server = {
        let server_name = server_name.clone();
        tokio::task::spawn_blocking(
            move || -> Result<deepseek_tui::mcp::McpServerConfig, String> {
                let mcp_path = crate::platform::paths::mcp_config_path();
                let content = crate::platform::filesystem::read_private_data_file(&mcp_path)
                    .map_err(|e| format!("读取 mcp.json 失败: {e}"))?;
                let config: deepseek_tui::mcp::McpConfig = serde_json::from_str(&content)
                    .map_err(|e| format!("解析 mcp.json 失败: {e}"))?;
                config
                    .servers
                    .get(&server_name)
                    .cloned()
                    .ok_or_else(|| format!("mcp.json 未找到服务 '{server_name}'"))
            },
        )
        .await
        .map_err(|e| format!("任务执行失败: {e}"))??
    };

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
        // The join-failure copy matches the neighboring commands' 任务执行失败
        // siblings (round-20 review; round-22 restored it after an
        // out-of-claim English flip had split the file's copy).
        .map_err(|e| format!("任务执行失败: {e}"))??;
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
    // Builtin plugins cannot be uninstalled (docs/builtin-toolset-contract.md
    // §3.1): fail fast at the command layer with a user-facing error; the
    // manager layer `MarketplaceManager::uninstall` carries the same guard
    // (defense in depth). Ids are normalized with `to_package_id` first, so a
    // `skill:`-prefixed alias of a builtin package is judged by its package.
    if crate::features::marketplace::builtin::is_builtin_tool(
        &crate::features::marketplace::scope::to_package_id(tool_id),
    ) {
        return Err(format!(
            "builtin plugin '{tool_id}' is part of the application and cannot be uninstalled"
        ));
    }
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
    // Teardown policy twin: this is the **eager pre-uninstall, abort-on-failure**
    // companion teardown. The post-commit best-effort twin lives in
    // features/marketplace::cleanup_uninstalled_tool_state (runs after the
    // uninstall transaction commits and swallows per-skill failures). Keep the
    // two policies distinct and the cross references intact when touching
    // either side.
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
    // Round-26 MAJOR 1 (review #455): snapshot every companion skill's owner
    // pack BEFORE any directory disappears — once the skill dir is deleted
    // (or the whole package is recycled), the normalized cleanup's gating
    // fallback can be re-owned by a foreign pack's claim/nesting and the
    // removal would erase THAT pack's consent rows.
    let companion_owners: std::collections::HashMap<String, String> = companions
        .iter()
        .map(|sid| {
            (
                sid.clone(),
                crate::features::marketplace::scope::resolve_pack_owner_id(sid),
            )
        })
        .collect();
    for sid in &companions {
        if recycles_with_package {
            continue; // companion 随整包回收，见上注释
        }
        let torn_down =
            crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
                .uninstall(sid)
                .map_err(|e| {
                    format!("联动卸载配套技能 '{sid}' 失败（已中止工具卸载，请重试）: {e}")
                })?;
        if !torn_down {
            // Vacuous companion uninstall (declared but never installed):
            // there is nothing legitimate to clear — stripping here would
            // remove a deny-first entry a concurrent install just registered
            // for that skill id. Leave every entry alone.
            continue;
        }
        // Scope entries are cleared only after the skill is actually gone —
        // otherwise a still-installed skill would be silently re-enabled.
        // Exact form (round-26 MAJOR 1): the rows were owned by the
        // pre-teardown snapshot; re-normalizing a deleted id is hijackable.
        // `sid` always has a snapshot entry (same iteration source), so the
        // fallback is unreachable; production code avoids expect().
        let owner = companion_owners.get(sid).map(String::as_str).unwrap_or(sid);
        crate::features::marketplace::scope::remove_bundle_from_disabled_scopes_exact(owner)?;
    }
    // The scope strips happen INSIDE the transaction (the in-lock leg in
    // cleanup_uninstalled_tool_state, gated on this call's real removal
    // outcome): there is deliberately no post-return strip here. A strip
    // after `mgr.uninstall` returned would run outside the transaction lock
    // and could undo the deny-first entry a concurrent same-id install
    // registered after our in-lock strip — re-enabling the package as it
    // lands (round-11 P2-1). The in-lock leg already covers both rows this
    // function used to re-strip post-return (the tool-id rows when the
    // record removal succeeded, and the recycled companions' rows via
    // strip_recycled_companions), so the strips below were pure window:
    // unreachable for the vacuous leg (it returns above) and able only to
    // erase a concurrent install's fresh registration on the non-vacuous
    // leg. A vacuous uninstall (the manager's `Ok(false)`) leaves every
    // entry alone: stranded leftover entries fail closed and do NOT converge
    // via install/uninstall — the deny-list composer's next full-list save
    // rewrites the file.
    let removed_install_record = mgr.uninstall(tool_id)?;
    if !removed_install_record {
        // Vacuous uninstall: no install record existed, so nothing was torn
        // down and the in-lock scope cleanup has nothing legitimate to
        // remove — but a post-return strip WOULD erase the deny-first
        // consent entry a concurrent same-id install may have just
        // registered (its install record lands after the gate), re-enabling
        // the package as it lands. Leave every entry alone.
        return Ok(());
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
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || install_marketplace_skill_sync(&skill_id))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))??;
    // The install affects both scopes' enabled sets: an initialized DenyAll
    // scope (plain included) keeps a newly installed skill off by default
    // (synced into the scope disabled sets, see install_marketplace_skill_sync);
    // an uninitialized scope falls back to the on-the-fly DenyAll expansion,
    // also off by default. The composite-dir rewrite is finalized by
    // hot_refresh below (round-20 minor 4: a hand-written
    // refresh_live_sessions_skills call used to duplicate what hot_refresh
    // already does — idempotent but wasted work).
    // The native-tool ownership gate (NATIVE_PACKAGE_TOOLS) reads install
    // state: without this refresh a package owning a native tool (ima) stays
    // denied in live engines until respawn.
    hot_refresh(&pool, true).await;
    Ok(())
}

/// DenyAll consent-gate registration for `install_marketplace_tool`, run
/// BEFORE the package lands (deny-first, #517 review round 4): a refusal
/// aborts the install before the package is exposed or an existing
/// installation is overwritten — nothing on disk has been touched yet.
pub(super) fn install_marketplace_tool_gates(tool_id: &str) -> Result<(), String> {
    // The sync itself skips known bundles (their consent is recorded), so a
    // reinstall never re-runs the write and never needs the lock.
    refuse_owner_claimed_install_id(tool_id)?;
    crate::features::marketplace::sync_deny_all_scopes_after_install(tool_id)
        .map_err(|refused| refused_sync_error(&format!("tool '{tool_id}'"), refused))
}

/// Owner-claim divergence refusal for the two by-name install gates (review
/// round 22, P1): the consent sync folds its id through the installed packs'
/// companion-skill vocabulary (`to_package_id`), and a DECLARED-but-unshipped
/// companion name survives import validation (`detect_components` checks only
/// shipped components) — so an installed mcp-only pack declaring
/// `companion_skills: ["weather"]` folds a later catalog install of "weather"
/// onto the claimant, hits the known-bundle skip, and lands the tool ENABLED
/// with zero consent rows in initialized DenyAll scopes (enforcement expands
/// only the ids stored in the disabled lists). The import channel refuses
/// this shape at its own boundary (the round-12 fold-divergence check); these
/// gates are the remaining by-name channels. The check is state-dependent on
/// purpose, like the import one: with no claimant installed the id self-maps
/// and installs normally, and a reinstall of the claimant's OWN pack id also
/// self-maps (its pack dir exists), so the known-skip reinstall contract is
/// untouched.
fn refuse_owner_claimed_install_id(id: &str) -> Result<(), String> {
    let folded = crate::features::marketplace::scope::to_package_id(id);
    if folded != id {
        return Err(format!(
            "'{id}' is claimed by installed pack '{folded}'s companion-skill vocabulary; \
             the consent gate would govern '{folded}', not '{id}' — uninstall '{folded}' first"
        ));
    }
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
    // copy in a rollback. The gate skips known bundles (their entries ARE
    // the recorded consent), so a reinstall never re-runs the write.
    // Round-16 (review): existence probe first — same rationale as the tool
    // install's probe (a garbage id must not seed phantom deny rows); same
    // lookup and error as the install's own validation.
    if !crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
        .preset_skill_exists(skill_id)
    {
        return Err(format!("unknown preset skill '{skill_id}'"));
    }
    // Same divergence refusal as the tool gate above: a preset skill id is
    // foldable too, and a claimant pack declaring it as an (unshipped)
    // companion would otherwise swallow the registration into its own
    // known-bundle skip.
    refuse_owner_claimed_install_id(skill_id)?;
    crate::features::marketplace::scope::sync_deny_all_scopes_after_install(skill_id)
        .map_err(|refused| refused_sync_error(&format!("skill '{skill_id}'"), refused))?;
    crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
        .install(skill_id)
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
        // DenyAll gate before the install replaces content (same seam as
        // every other install channel): for a genuinely installed preset the
        // known-bundle skip returns `Ok` without writing — its entries ARE
        // the recorded consent — so today this is a no-op. It stays here so
        // the no-exposure property is enforced by the gate itself, not by
        // the list precondition above silently holding (round-11 P3).
        crate::features::marketplace::sync_deny_all_scopes_after_install(&skill_id)
            .map_err(|refused| refused_sync_error(&format!("skill '{skill_id}'"), refused))?;
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
    pre_land: &dyn Fn(&str, &[String]) -> Result<(), String>,
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
/// `skills` are the package's identified skill components: one no installed
/// manifest claims will materialize as its own standalone package (the owner
/// claim falls through to the component id), so the package-level entry
/// never covers it — standalone components are registered deny-first too,
/// or an undeclared component would land enabled in initialized DenyAll
/// scopes (round-11 P2-3). Components this package itself declares are
/// covered by the package entry once its record lands (read-time
/// normalization folds the claim onto the package id); a component claimed
/// by a foreign installed package normalizes to that owner, whose entry
/// covers it. Uninitialized DenyAll scopes are not a gap: their default
/// deny set also unions the `bundles/*/skills/` disk walk through the
/// physical gating owner (`skill_gating_owner_with`), so a landed
/// standalone component resolves to its own pack id and sits under the
/// default-full-deny (round-17 review — the earlier "preset/upload-record
/// ids only" residual no longer existed).
fn deny_all_pre_land(id: &str, skills: &[String]) -> Result<(), String> {
    crate::features::marketplace::sync_deny_all_scopes_after_install(id)
        .map_err(|refused| refused_sync_error(&format!("package '{id}'"), refused))?;
    // A component no installed manifest claims materializes as its own
    // standalone package (owner claim falls through to the component id), so
    // the package-level entry never covers it — standalone components are
    // registered deny-first too, or an undeclared component would land
    // enabled in initialized DenyAll scopes (round-11 P2-3). One exception
    // keeps reimports honest: a component already materialized under an
    // INSTALLED copy of this same package ships with it, so its consent is
    // the package's recorded state (the known-skip above covered the package
    // id) — re-registering it would re-deny recorded consent on every
    // conflict-rejected reimport and mask the content-conflict refusal
    // behind a lock refusal. Without the installed-record requirement, an
    // uninstalled custom package's kept dir beside a stale `installed=false`
    // record would suppress a genuinely fresh registration.
    let record_installed = crate::features::marketplace::store::BundleStore::new()
        .get(id)
        .ok()
        .flatten()
        .map(|record| record.installed)
        .unwrap_or(false);
    // Round-16 (review): one manifest snapshot serves every owner-claim
    // check below — the per-skill wrapper re-walked all manifests per
    // component. Round-19: the hoist now also covers the package-id
    // resolution itself (the loop still called the walking `to_package_id`).
    let tools = crate::features::marketplace::MarketplaceManager::new().available_tools();
    for skill in skills {
        let owner = crate::features::marketplace::bundle::skill_owner_package_with(&tools, skill);
        if owner != *skill {
            continue;
        }
        let package_id = crate::features::marketplace::scope::to_package_id_with(&tools, skill);
        if package_id == id {
            continue;
        }
        if record_installed
            && crate::platform::paths::bundles_root()
                .join(id)
                .join("skills")
                .join(skill)
                .join("SKILL.md")
                .is_file()
        {
            continue;
        }
        crate::features::marketplace::sync_deny_all_scopes_after_install(&package_id)
            .map_err(|refused| refused_sync_error(&format!("skill '{package_id}'"), refused))?;
    }
    Ok(())
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
    // 上传安全默认：插件包导入后加入 DenyAll 禁用集，需用户在前端开关显式开启。
    // Same contract as install_marketplace_tool. Fail-visible persist (review #455 R13-B3).
    // Off the executor: the sync waits on the cross-process scope flock,
    // which a frozen peer holds indefinitely.
    let imported_id = report.id.clone();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::sync_deny_all_scopes_after_install(&imported_id)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e| {
        format!(
            "plugin '{}' installed, but {}: new sessions will enable it by default — turn it off in the tools list: {e}",
            report.id,
            crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
        )
    })?;
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

/// Drag-and-drop plugin import (unified upload, same semantics as
/// `import_plugin_package_cmd`): the frontend reads the zip into base64 and
/// passes it here; it is staged to a temp file and goes through
/// `import_plugin_package_sync` (the unified pre-land gate pipeline).
/// Returns the new pack id = imported (the frontend opens the
/// display-info edit dialog for it).
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
    // 上传安全默认：拖放导入插件包后加入 DenyAll 禁用集，需用户开关显式开启。
    // Fail-visible persist (review #455 R13-B3). Off the executor: the sync
    // waits on the cross-process scope flock, which a frozen peer holds
    // indefinitely.
    let imported_id = report.id.clone();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::sync_deny_all_scopes_after_install(&imported_id)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e| {
        format!(
            "plugin '{}' installed, but {}: new sessions will enable it by default — turn it off in the tools list: {e}",
            report.id,
            crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
        )
    })?;
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
    let report = tokio::task::spawn_blocking(move || {
        import_skill_md_content_gated(md, &filename_for_import)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    // Upload safe default: same as plugin import, joins the DenyAll scopes.
    // Fail-visible persist (review #455 R13-B3). Off the executor: the sync
    // waits on the cross-process scope flock, which a frozen peer holds
    // indefinitely.
    let imported_id = report.id.clone();
    tokio::task::spawn_blocking(move || {
        crate::features::marketplace::scope::sync_deny_all_scopes_after_install(&imported_id)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e| {
        format!(
            "skill '{}' installed, but {}: new sessions will enable it by default — turn it off in the tools list: {e}",
            report.id,
            crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
        )
    })?;
    // An imported id can collide with a package owning a native tool
    // (NATIVE_PACKAGE_TOOLS keys on package ids), so the deny snapshot must
    // follow the same install postcondition as the marketplace paths.
    // The composite-dir rewrite is finalized by hot_refresh (round-20 minor 4:
    // the duplicate hand-written refresh_live_sessions_skills call is gone).
    hot_refresh(&pool, true).await;
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
    // Round-26 MAJOR 1 (review #455): snapshot the owner pack while the skill
    // dir is still on disk — after the deletion the normalized cleanup's
    // gating fallback could be hijacked by a foreign pack's claim/nesting and
    // erase THAT pack's consent rows.
    let owner = crate::features::marketplace::scope::resolve_pack_owner_id(skill_id);
    let torn_down = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
        .uninstall(skill_id)?;
    if !torn_down {
        // Vacuous skill uninstall (no record, nothing on disk): there is
        // nothing legitimate to clear — stripping here would remove the
        // deny-first consent entry a concurrent same-id install just
        // registered (its install record lands after the gate) and re-enable
        // the skill as it lands.
        return Ok(());
    }
    // 已卸载的技能从两个 scope 的禁用集移除（避免残留 id，与连接器同语义）；
    // exact 形式按卸载前快照的属主清行（round-26 MAJOR 1）。
    crate::features::marketplace::scope::remove_bundle_from_disabled_scopes_exact(&owner)?;
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
    /// Action dispatch (§3.1): the action set is derived server-side from the
    /// current state. serde default keeps the contract purely additive; the
    /// frontend switch to the action renderer lands in a later PR.
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
        // Delegated to the shared RAII helper (round 9): a failing assertion
        // unwinds past a straight-line env restore, which would leave
        // PINVOU3_HOME pointed at a deleted temp dir and cascade unrelated
        // failures for every later test in the process.
        crate::platform::test_support::with_temp_home("pinvou3-gate-rb", f);
    }

    /// The Upload-recycle uninstall strips the owner's and the companions'
    /// deny entries INSIDE the transaction (cleanup_uninstalled_tool_state
    /// under MARKETPLACE_TRANSACTION_LOCK, round-11 P2-1): no scope write
    /// happens after `mgr.uninstall` returns, so a deny-first entry a
    /// concurrent same-id install registers past that point survives. The
    /// entries here are seeded directly (a save while the package is
    /// installed would read-time-normalize the companion id onto the owner);
    /// the seed id must survive both strips.
    #[test]
    fn uninstall_upload_recycle_strips_owner_and_companion_entries_in_lock() {
        with_temp_home(|| {
            // Combo on disk + Upload record + Upload install record, mirroring
            // the recycle fixture of
            // uninstall_upload_bundle_via_command_recycles_companion_skills.
            let manifest_dir =
                crate::features::marketplace::mcp_catalog::package_mcp_dir("up-lock");
            std::fs::create_dir_all(&manifest_dir).unwrap();
            std::fs::write(
                manifest_dir.join("manifest.json"),
                r#"{"id":"up-lock","name":"UpLock","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["up-lock-skill"]}"#,
            )
            .unwrap();
            let skill_dir =
                crate::platform::paths::bundles_root().join("up-lock/skills/up-lock-skill");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: up-lock-skill\n---\n",
            )
            .unwrap();
            let mgr = crate::features::marketplace::MarketplaceManager::new();
            mgr.install_upload(
                "up-lock",
                crate::features::marketplace::store::BundleSource::Upload(
                    "up-lock.zip".to_string(),
                ),
            )
            .unwrap();

            // Seed consent state directly: owner id, standalone-era companion
            // id, and an unrelated entry that must survive.
            let disabled = crate::platform::paths::pinvou3_home().join("disabled_bundles.json");
            std::fs::write(
                &disabled,
                r#"{"scopes":{"code":["seed-bundle","up-lock","up-lock-skill"]},"initialized":["code"]}"#,
            )
            .unwrap();

            uninstall_marketplace_tool_sync("up-lock").expect("uninstall must succeed");

            let file: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&disabled).unwrap()).unwrap();
            assert_eq!(
                file["scopes"]["code"],
                serde_json::json!(["seed-bundle"]),
                "owner and companion entries must be stripped by the in-lock leg; \
                 unrelated entries must survive"
            );
            assert_eq!(
                file["initialized"].as_array().unwrap().len(),
                1,
                "the strip must not touch the scope's initialization marker"
            );
        });
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
    /// store record. Helper-granularity pin — the end-to-end command wiring
    /// (gate before `MarketplaceManager::install`) is pinned by
    /// `install_tool_sync_refused_before_anything_lands`.
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

    /// The deny-first ORDERING of the tool install path, end-to-end: the
    /// command's sync core must run the consent gate BEFORE
    /// `MarketplaceManager::install`, so a refused registration lands nothing
    /// at all. Driving the gate helper in isolation
    /// (`install_tool_gates_refuse_before_anything_lands`) would stay green
    /// if the command were reverted to install-first-then-gate. The id is a
    /// REAL catalog package ("weather"): the round-16 existence probe passes,
    /// the fixed world refuses at the gate (the error names the lock), and
    /// the dir/record asserts below are what catch a reverted ordering — an
    /// install-first shape would land the weather package before the gate
    /// refused. (Round-15's unknown-id probe of this shape, "gate-wiring-
    /// probe", became an ordering-blind shortcut when the round-16 existence
    /// probe moved ahead of the gate: a garbage id now fails with the
    /// catalog error before any lock is consulted.)
    #[test]
    fn install_tool_sync_refused_before_anything_lands() {
        with_temp_home(|| {
            init_code_scope_then_break_lock();

            let error = install_marketplace_tool_sync("weather", &Default::default()).unwrap_err();
            assert!(
                error.contains("disabled_bundles.lock"),
                "the refusal must come from the consent gate and name the lock \
                 failure, not from a post-gate step: {error}"
            );
            assert!(
                !crate::platform::paths::bundles_root()
                    .join("weather")
                    .exists(),
                "a refused install must not create the package dir"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("weather")
                    .unwrap()
                    .is_none(),
                "a refused install must not write an install record"
            );
        });
    }

    /// Review round 22 (P1): an installed pack's DECLARED companion
    /// vocabulary must not hijack a later by-name install. An mcp-only pack
    /// whose manifest declares an unshipped `companion_skills` entry (import
    /// validation checks only shipped components) folds the claimed id onto
    /// the claimant inside the consent sync; without the divergence refusal
    /// the gate hits the claimant's known-bundle skip, registers nothing, and
    /// the tool/preset lands ENABLED with zero consent rows in initialized
    /// DenyAll scopes (enforcement expands only the stored ids). Both lanes
    /// are pinned with a WORKING lock so the only possible refusal source is
    /// the divergence check, plus the positive control: once the claimant is
    /// gone the id self-maps and the gate registers deny-first.
    #[test]
    fn install_gates_refuse_companion_claimed_ids() {
        with_temp_home(|| {
            let manifest_dir = crate::platform::paths::bundles_root().join("evil/mcp");
            std::fs::create_dir_all(&manifest_dir).unwrap();
            std::fs::write(
                manifest_dir.join("manifest.json"),
                r#"{"id":"evil","name":"Evil","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"python","args":["s.py"],"companion_skills":["weather","pptx"]}"#,
            )
            .unwrap();
            let store = crate::features::marketplace::store::BundleStore::new();
            store
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        "evil",
                        crate::features::marketplace::store::BundleSource::Upload(
                            "Evil".to_string(),
                        ),
                    ),
                )
                .unwrap();
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &["seed-bundle".to_string()],
            )
            .expect("code scope must initialize while the lock works");

            let tool_error = install_marketplace_tool_gates("weather").unwrap_err();
            assert!(
                tool_error.contains("evil") && tool_error.contains("companion-skill"),
                "the refusal must name the claimant pack: {tool_error}"
            );
            assert!(
                !tool_error.contains("disabled_bundles.lock"),
                "the refusal is the divergence check, not a lock failure: {tool_error}"
            );

            let skill_error = install_marketplace_skill_sync("pptx").unwrap_err();
            assert!(
                skill_error.contains("evil"),
                "the preset lane must refuse the claimed name too: {skill_error}"
            );
            assert!(
                crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
                    .find_skill_dir("pptx")
                    .is_none(),
                "a refused preset install must not land the skill"
            );

            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "the claimed ids must gain no deny rows (registration never ran)"
            );

            // Positive control: the fold is state-dependent — with the
            // claimant gone the id self-maps and the gate registers.
            std::fs::remove_dir_all(crate::platform::paths::bundles_root().join("evil")).unwrap();
            store.remove("evil").unwrap();
            install_marketplace_tool_gates("weather")
                .expect("with no claimant installed the id self-maps and the gate runs");
            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string(), "weather".to_string()],
                "the fresh id must be registered deny-first once unclaimed"
            );
        });
    }

    /// Round-16 (review): a garbage direct-IPC id must fail on the existence
    /// probe BEFORE the deny-first gate — otherwise the gate seeds phantom
    /// deny rows, default-off markers and ledger entries for a package that
    /// cannot exist (pure over-denial surviving until the next composer
    /// full-list write, and a dead ledger entry suppressing any future
    /// startup backfill). Both install lanes are pinned: the error is the
    /// catalog's own, and the persisted consent state is untouched.
    #[test]
    fn install_unknown_ids_write_no_consent_state() {
        with_temp_home(|| {
            init_code_scope_then_break_lock();

            let tool_error =
                install_marketplace_tool_sync("definitely-not-a-tool", &Default::default())
                    .unwrap_err();
            assert!(
                tool_error.contains("does not exist"),
                "the tool probe must fail with the catalog's own error: {tool_error}"
            );
            assert!(
                !tool_error.contains("disabled_bundles.lock"),
                "the gate must not have run for a nonexistent id: {tool_error}"
            );

            let skill_error = install_marketplace_skill_sync("definitely-not-a-skill").unwrap_err();
            assert!(
                skill_error.contains("unknown preset skill"),
                "the skill probe must fail with the install's own error: {skill_error}"
            );
            assert!(
                !skill_error.contains("disabled_bundles.lock"),
                "the gate must not have run for a nonexistent skill: {skill_error}"
            );

            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "the initialized deny state must gain no phantom rows"
            );
            assert!(
                crate::features::marketplace::scope::load_disabled_bundles_file()
                    .install_default_synced
                    .is_empty(),
                "the sync ledger must gain no dead entries"
            );
        });
    }

    /// The unified plugin-package import channel's deny-first ordering,
    /// end-to-end through the production entry (`import_plugin_package_sync`):
    /// a FRESH import whose consent-gate registration is refused (broken
    /// lock) must land nothing — no package dir, no store record. The
    /// sibling re-import test pins the installed-package half (conflict
    /// refusal); without this test, moving the pre-land hook after the
    /// staging step would stay green.
    #[test]
    fn import_fresh_refused_before_landing() {
        with_temp_home(|| {
            init_code_scope_then_break_lock();

            let plugin_json = r#"{"manifest_version":1,"id":"gate-fresh-plugin","name":"p","components":{"skills":[{"id":"gate-fresh-skill","dir":"skills/gate-fresh-skill"}]}}"#;
            let mut zip_buf = std::io::Cursor::new(Vec::new());
            {
                use std::io::Write;
                let mut zw = zip::ZipWriter::new(&mut zip_buf);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("plugin.json", opts).unwrap();
                zw.write_all(plugin_json.as_bytes()).unwrap();
                zw.start_file("skills/gate-fresh-skill/SKILL.md", opts)
                    .unwrap();
                zw.write_all(b"---\nname: gate-fresh-skill\ndescription: fresh\n---\nbody")
                    .unwrap();
                zw.finish().unwrap();
            }
            let tmp = std::env::temp_dir().join(format!(
                "gate-fresh-plugin-{}-{}.zip",
                std::process::id(),
                crate::platform::paths::tests::unique_suffix()
            ));
            std::fs::write(&tmp, zip_buf.into_inner()).unwrap();

            let error = import_plugin_package_sync(&tmp.to_string_lossy(), "gate-fresh-plugin.zip")
                .unwrap_err();
            assert!(
                error.contains("disabled_bundles.lock"),
                "the refusal must come from the pre-land consent gate and name the \
                 lock failure: {error}"
            );
            assert!(
                !crate::platform::paths::bundles_root()
                    .join("gate-fresh-plugin")
                    .exists(),
                "a refused fresh import must not create the package dir"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("gate-fresh-plugin")
                    .unwrap()
                    .is_none(),
                "a refused fresh import must not write an install record"
            );
            let _ = std::fs::remove_file(&tmp);
        });
    }

    /// Upload channels must not be able to reserve a preset skill's MARKET
    /// id (round-11 P2-2): install records and deny entries are keyed under
    /// market ids, so a colliding upload's record would make the consent
    /// gate treat the real preset as already known (its install skips
    /// registration — fail-open) and read-time normalization would fold the
    /// preset's alias vocabulary onto the upload. Both directions are
    /// reserved: the package id against preset market ids AND preset skill
    /// names, and every component name against both vocabularies.
    #[test]
    fn import_rejects_preset_market_id_collisions() {
        with_temp_home(|| {
            let build_zip = |id: &str, skill_id: &str| {
                let plugin_json = format!(
                    r#"{{"manifest_version":1,"id":"{id}","name":"p","components":{{"skills":[{{"id":"{skill_id}","dir":"skills/{skill_id}"}}]}}}}"#
                );
                let mut zip_buf = std::io::Cursor::new(Vec::new());
                {
                    use std::io::Write;
                    let mut zw = zip::ZipWriter::new(&mut zip_buf);
                    let opts = zip::write::SimpleFileOptions::default();
                    zw.start_file("plugin.json", opts).unwrap();
                    zw.write_all(plugin_json.as_bytes()).unwrap();
                    zw.start_file(format!("skills/{skill_id}/SKILL.md"), opts)
                        .unwrap();
                    zw.write_all(
                        format!("---\nname: {skill_id}\ndescription: c\n---\nbody").as_bytes(),
                    )
                    .unwrap();
                    zw.finish().unwrap();
                }
                let tmp = std::env::temp_dir().join(format!(
                    "preset-id-collide-{}-{}.zip",
                    std::process::id(),
                    crate::platform::paths::tests::unique_suffix()
                ));
                std::fs::write(&tmp, zip_buf.into_inner()).unwrap();
                tmp
            };

            // The package id itself is the aliased preset's market id
            // (`tencent-docs-skill` ↔ skill name `tencent-docs`): the case
            // the name-only reservation missed.
            let tmp = build_zip("tencent-docs-skill", "own-skill");
            let error =
                import_plugin_package_sync(&tmp.to_string_lossy(), "collide.zip").unwrap_err();
            assert!(
                error.contains("marketplace preset skill"),
                "a package id colliding with a preset market id must be refused: {error}"
            );
            let _ = std::fs::remove_file(&tmp);

            // The reverse direction: a component name keyed under a preset
            // market id (a standalone skill would normalize to itself and
            // hijack the same vocabulary).
            let tmp = build_zip("own-collide-pkg", "tencent-docs-skill");
            let error =
                import_plugin_package_sync(&tmp.to_string_lossy(), "collide2.zip").unwrap_err();
            assert!(
                error.contains("marketplace preset skill"),
                "a component name colliding with a preset market id must be refused: {error}"
            );
            let _ = std::fs::remove_file(&tmp);
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("tencent-docs-skill")
                    .unwrap()
                    .is_none()
                    && crate::features::marketplace::store::BundleStore::new()
                        .get("own-collide-pkg")
                        .unwrap()
                        .is_none(),
                "refused collisions must not write install records"
            );
        });
    }

    /// Round-12 review B1: a package id that `to_package_id` folds onto an
    /// INSTALLED claimant's consent vocabulary must be refused. With gongwen
    /// installed, the id `government-writing` (its declared companion skill
    /// name) folds onto `gongwen` inside the consent gate, so the gate's
    /// known-skip fires on the CLAIMANT and a package imported under the id
    /// would land with no deny entry of its own — ungoverned in initialized
    /// DenyAll scopes. The refusal must name the owning package. The second
    /// half pins that the rejection is state-dependent, not a blanket ban:
    /// with the claimant uninstalled the same id self-maps, the import
    /// succeeds, and the fresh id is registered deny-first — the export →
    /// re-import round-trip contract.
    #[test]
    fn import_rejects_owner_claimed_package_ids() {
        with_temp_home(|| {
            // Seed the gongwen claimant directly as an installed store record
            // instead of running the real preset install: the fold
            // (`skill_owner_package` → `bundle_installed`) consults the
            // embedded catalog manifest plus the store record only, and the
            // real install would run gongwen's legacy pip fallback
            // (python-docx) on non-Windows hosts — network-dependent in CI.
            crate::features::marketplace::store::BundleStore::new()
                .upsert(crate::features::marketplace::store::BundleRecord {
                    id: "gongwen".to_string(),
                    source: crate::features::marketplace::store::BundleSource::Preset,
                    installed: true,
                    content_fingerprint: Some("fp".to_string()),
                    installed_at: "2026-08-20T00:00:00+00:00".to_string(),
                    degraded: None,
                    assets: Vec::new(),
                    extra: serde_json::Map::new(),
                })
                .expect("seed gongwen install record");
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &["seed-bundle".to_string()],
            )
            .expect("code scope must initialize");

            let manifest = r#"{"id":"government-writing","name":"Government Writing Plus","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":["draft_doc"],"command":"python","args":["server.py"]}"#;
            let mut zip_buf = std::io::Cursor::new(Vec::new());
            {
                use std::io::Write;
                let mut zw = zip::ZipWriter::new(&mut zip_buf);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("mcp/manifest.json", opts).unwrap();
                zw.write_all(manifest.as_bytes()).unwrap();
                zw.start_file("mcp/server.py", opts).unwrap();
                zw.write_all(b"print('attacker')").unwrap();
                zw.finish().unwrap();
            }
            let tmp = std::env::temp_dir().join(format!(
                "owner-claim-id-{}-{}.zip",
                std::process::id(),
                crate::platform::paths::tests::unique_suffix()
            ));
            std::fs::write(&tmp, zip_buf.into_inner()).unwrap();

            let error =
                import_plugin_package_sync(&tmp.to_string_lossy(), "government-writing.zip")
                    .unwrap_err();
            assert!(
                error.contains("gongwen"),
                "the refusal must name the owning package: {error}"
            );
            assert!(
                !crate::platform::paths::bundles_root()
                    .join("government-writing")
                    .exists(),
                "a refused owner-claimed import must not create the package dir"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("government-writing")
                    .unwrap()
                    .is_none(),
                "a refused owner-claimed import must not write an install record"
            );
            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "the refused import must leave the deny list unchanged"
            );

            // Round-trip half: with the claimant's record gone, the same id
            // self-maps, so the SAME zip must import — and its own id must be
            // deny-first registered by the gate.
            crate::features::marketplace::store::BundleStore::new()
                .remove("gongwen")
                .expect("remove the seeded gongwen record");
            import_plugin_package_sync(&tmp.to_string_lossy(), "government-writing.zip")
                .expect("without the installed claimant the id self-maps and must import");
            assert!(
                crate::platform::paths::bundles_root()
                    .join("government-writing")
                    .exists(),
                "the round-trip import must land its package dir"
            );
            let deny_list = crate::features::marketplace::load_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
            );
            assert!(
                deny_list.iter().any(|id| id == "government-writing"),
                "the fresh id must be deny-first registered once the claimant is gone: {deny_list:?}"
            );
            let _ = std::fs::remove_file(&tmp);
        });
    }

    /// Round-12 review B3 (import side): a case-variant spelling of a builtin
    /// CLI connector id must be refused. `cli_bundle_skill_dirs` matches the
    /// exact id only, so on a case-insensitive filesystem an id like
    /// `Dingtalk` would land at the same physical path as the connector's
    /// real companion skill dirs — the guard must also fold case against the
    /// builtin connector id list.
    #[test]
    fn import_rejects_case_variant_connector_ids() {
        with_temp_home(|| {
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &["seed-bundle".to_string()],
            )
            .expect("code scope must initialize");

            let manifest = r#"{"id":"Dingtalk","name":"Dingtalk Plus","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":["send_message"],"command":"python","args":["server.py"]}"#;
            let mut zip_buf = std::io::Cursor::new(Vec::new());
            {
                use std::io::Write;
                let mut zw = zip::ZipWriter::new(&mut zip_buf);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("mcp/manifest.json", opts).unwrap();
                zw.write_all(manifest.as_bytes()).unwrap();
                zw.start_file("mcp/server.py", opts).unwrap();
                zw.write_all(b"print('attacker')").unwrap();
                zw.finish().unwrap();
            }
            let tmp = std::env::temp_dir().join(format!(
                "case-variant-cli-{}-{}.zip",
                std::process::id(),
                crate::platform::paths::tests::unique_suffix()
            ));
            std::fs::write(&tmp, zip_buf.into_inner()).unwrap();

            let error =
                import_plugin_package_sync(&tmp.to_string_lossy(), "Dingtalk.zip").unwrap_err();
            assert!(
                error.contains("CLI"),
                "a case-variant builtin CLI connector id must hit the CLI-collision refusal: {error}"
            );
            assert!(
                !crate::platform::paths::bundles_root()
                    .join("Dingtalk")
                    .exists(),
                "a refused case-variant import must not create the package dir"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .get("Dingtalk")
                    .unwrap()
                    .is_none(),
                "a refused case-variant import must not write an install record"
            );
            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "the refused import must leave the deny list unchanged"
            );
            let _ = std::fs::remove_file(&tmp);
        });
    }

    /// A combo package whose MCP manifest omits `companion_skills` leaves
    /// its skill components standalone (the owner claim falls through to the
    /// component id), so the package-level deny entry never covers them —
    /// the gate must register each standalone component deny-first too, or
    /// an undeclared component lands enabled in initialized DenyAll scopes
    /// (round-11 P2-3). The declared twin shows the package entry covering a
    /// claimed component via read-time normalization.
    #[test]
    fn import_registers_standalone_components_deny_first() {
        with_temp_home(|| {
            // Initialize the code scope so the DenyAll gate has somewhere to
            // write; the seed id keeps the assertion discriminating.
            crate::features::marketplace::scope::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &["seed-bundle".to_string()],
            )
            .unwrap();

            let build_zip = |id: &str, skill_id: &str, with_manifest: bool| {
                let plugin_json = format!(
                    r#"{{"manifest_version":1,"id":"{id}","name":"p","components":{{"skills":[{{"id":"{skill_id}","dir":"skills/{skill_id}"}}]}}}}"#
                );
                let mut zip_buf = std::io::Cursor::new(Vec::new());
                {
                    use std::io::Write;
                    let mut zw = zip::ZipWriter::new(&mut zip_buf);
                    let opts = zip::write::SimpleFileOptions::default();
                    zw.start_file("plugin.json", opts).unwrap();
                    zw.write_all(plugin_json.as_bytes()).unwrap();
                    zw.start_file(format!("skills/{skill_id}/SKILL.md"), opts)
                        .unwrap();
                    zw.write_all(
                        format!("---\nname: {skill_id}\ndescription: c\n---\nbody").as_bytes(),
                    )
                    .unwrap();
                    if with_manifest {
                        // The MCP manifest declares the component as a
                        // companion: after the record lands, read-time
                        // normalization folds the component onto the
                        // package id.
                        let manifest = format!(
                            r#"{{"id":"{id}","name":"{id}","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["{skill_id}"]}}"#
                        );
                        zw.start_file("mcp/manifest.json", opts).unwrap();
                        zw.write_all(manifest.as_bytes()).unwrap();
                    }
                    zw.finish().unwrap();
                }
                let tmp = std::env::temp_dir().join(format!(
                    "standalone-component-{}-{}.zip",
                    std::process::id(),
                    crate::platform::paths::tests::unique_suffix()
                ));
                std::fs::write(&tmp, zip_buf.into_inner()).unwrap();
                tmp
            };
            let deny_list = || {
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code,
                )
            };

            // Undeclared component: the component id must carry its own
            // deny-first entry beside the package's, or the skill
            // materializes enabled.
            let tmp = build_zip("orphan-combo", "orphan-guide", false);
            import_plugin_package_sync(&tmp.to_string_lossy(), "orphan-combo.zip").unwrap();
            let _ = std::fs::remove_file(&tmp);
            let list = deny_list();
            assert!(
                list.iter().any(|id| id == "orphan-combo"),
                "the package id must be deny-first registered: {list:?}"
            );
            // Merged-world note: the deny-first entry for the undeclared
            // component IS written pre-land (the sync ledger proves it), but
            // the persisted row is then folded onto the package id — the
            // component dir lands physically nested under the package, and
            // the R17-MAJOR1 physical-aware owner maps "orphan-guide" to
            // "orphan-combo" at the next normalization write. Coverage is the
            // invariant that matters: the normalized view is governed through
            // the package entry, and the ledger pins that the gate ran for
            // the component id itself.
            let raw = crate::features::marketplace::scope::load_disabled_bundles_file();
            assert!(
                raw.install_default_synced
                    .iter()
                    .any(|k| k.ends_with(":orphan-guide")),
                "the undeclared component must have been deny-first registered (ledger): {raw:?}"
            );

            // Declared component: the package entry covers it (the
            // component's claim folds onto the package id once the record
            // lands), so no separate standalone entry survives normalization.
            let tmp = build_zip("claimed-combo", "claimed-guide", true);
            import_plugin_package_sync(&tmp.to_string_lossy(), "claimed-combo.zip").unwrap();
            let _ = std::fs::remove_file(&tmp);
            let list = deny_list();
            assert!(
                list.iter().any(|id| id == "claimed-combo"),
                "the declared combo must be deny-first registered: {list:?}"
            );
            assert!(
                !list.iter().any(|id| id == "claimed-guide"),
                "a declared component normalizes onto the package id and must \
                 not leave a separate standalone entry: {list:?}"
            );
        });
    }

    /// The wrapped-.md upload channel's deny-first wiring (round 9). The zip
    /// sibling above pins the unified pipeline through
    /// `import_plugin_package_sync`; without this test, reverting the `.md`
    /// command's gated wrapper to the ungated pipeline — or moving its
    /// pre-land hook after the landing — kept the entire suite green.
    /// Content assertions go through the bundles-root listing, which is
    /// robust to the bare-skill id derivation (frontmatter name vs hashed
    /// fallback): any landing changes it, any refusal leaves it untouched.
    #[test]
    fn import_skill_md_refused_before_landing() {
        with_temp_home(|| {
            init_code_scope_then_break_lock();

            let bundles_root = crate::platform::paths::bundles_root();
            std::fs::create_dir_all(&bundles_root).unwrap();
            let listing = || {
                let mut names: Vec<String> = std::fs::read_dir(&bundles_root)
                    .unwrap()
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names
            };
            let before = listing();

            let error = import_skill_md_content_gated(
                "---\nname: gate-md-skill\ndescription: md upload\n---\nbody".to_string(),
                "gate-md-skill.md",
            )
            .unwrap_err();
            assert!(
                error.contains("disabled_bundles.lock"),
                "the refusal must come from the pre-land consent gate and name the \
                 lock failure: {error}"
            );
            assert_eq!(
                listing(),
                before,
                "a refused .md upload must land no package dir"
            );
            assert!(
                crate::features::marketplace::store::BundleStore::new()
                    .records()
                    .unwrap()
                    .is_empty(),
                "a refused .md upload must not write an install record"
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

    /// Review round 4 regression, final semantics, three halves. (1) With the
    /// lock WORKING, a same-content re-import must skip the consent-gate
    /// write entirely: a regression to the old unconditional registration
    /// re-denies the landed package HERE and fails the deny-list assert —
    /// the observation a broken-lock fixture can never make (with the lock
    /// gone, fix-present, fully-reverted, and known-skip-removed are
    /// observationally identical; round-20 P3 restored this half — it had
    /// been lost in the #455 convergence rewrite). (2) With the lock BROKEN
    /// the refused gate must leave the pre-existing copy and deny state
    /// untouched (the pre-deny-first rollback destroyed the user's copy
    /// outright). Companion handling is best-effort by contract, so half (2)
    /// cannot discriminate — it pins the no-destruction property. Final
    /// semantics for the unified plugin-package import channel: the consent
    /// gate runs before the same-content check but SKIPS installed bundles,
    /// so a rejected re-import hits the content-conflict refusal instead of
    /// silently flipping the package's consent state — the v1 content and
    /// its enabled state stay intact.
    #[test]
    fn reimport_conflict_preserves_consent_state() {
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

            // Half (1): working lock. Initialize the Code scope, then drive a
            // same-content re-import through the gate — the known-clause must
            // skip the consent write entirely.
            crate::features::marketplace::save_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Code,
                &["seed-bundle".to_string()],
            )
            .expect("code scope must initialize while the lock works");
            let report_again =
                import_plugin_package_sync(&v1.to_string_lossy(), "gate-rb-plugin.zip")
                    .expect("a same-content reimport must succeed while the lock works");
            assert_eq!(report_again.id, "gate-rb-plugin");
            assert_eq!(
                crate::features::marketplace::load_disabled_bundles_for(
                    crate::features::marketplace::ConnectorScope::Code
                ),
                vec!["seed-bundle".to_string()],
                "a known-bundle reimport must not write consent state (working lock)"
            );

            init_code_scope_then_break_lock();

            let error = import_plugin_package_sync(&v2.to_string_lossy(), "gate-rb-plugin.zip")
                .unwrap_err();
            assert!(
                error.contains("gate-rb-plugin") && !error.contains("disabled_bundles"),
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
