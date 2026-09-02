use super::prelude::*;
use crate::features::assistant::engine_pool::CancelOutcome;

// ===================== 阶段 C: 取消生成 + 编辑/重发 =====================

/// Cancel the current generation (the ⏹ stop button during generation;
/// interrupt-and-jump-the-queue goes through here as well).
/// engine 立即 cancel_token.cancel()，turn loop 跳出后会发 TurnComplete 事件，
/// 前端通过 chat:done 解锁 busy 状态；若 engine 已不存在但池仍记录活动 turn，
/// EnginePool 会补发 Interrupted 终态。空闲会话取消保持 no-op。
///
/// Returns [`CancelOutcome`]: `generation` = the cancelled turn's epoch,
/// `terminal` = whether the target turn's terminal is confirmed. The
/// frontend's interruptAndSend uses it to decide whether to wait for
/// chat:done — closing the deterministic race where "the claim path emits
/// the terminal before the cancel command returns and the frontend listener
/// always misses it", plus the window where "the turn just ended naturally,
/// the cancel is a no-op and no event follows".
///
/// `keep_inbox` (P0-A): interrupts (⚡) pass true — un-injected steers are
/// kept for the next turn; stop (⏹) defaults to false — un-injected steers
/// are cleared with `chat:steer_dropped` emitted, the frontend removes the
/// queued chip with a notice, and the message cannot hang in a "gone from
/// the UI but alive in the engine" state.
#[tauri::command]
pub async fn cancel_generation(
    session_id: Option<String>,
    keep_inbox: Option<bool>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<CancelOutcome, String> {
    // 多 session:取消指定 session(前端传 session_id);兼容旧前端回退 active。
    if let Some(sid) = session_id.or_else(|| store.active_id()) {
        log::info!("[pinvou3][chat] cancel requested sid={}", sid);
        let outcome = pool.cancel(&sid, keep_inbox.unwrap_or(false)).await;
        log::info!("[pinvou3][chat] cancel command completed sid={}", sid);
        Ok(outcome)
    } else {
        log::warn!("[pinvou3][chat] cancel requested but no session id is available");
        Ok(CancelOutcome {
            generation: None,
            terminal: true,
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilities {
    pub os: &'static str,
    pub show_megacube_site: bool,
    pub show_super_permission_settings: bool,
    pub uses_bundled_dependency_installer: bool,
    pub uses_homebrew_dependency_installer: bool,
    pub task_completion_notifications_default: bool,
    pub local_vllm_supported: bool,
    pub codex_acp_supported: bool,
    pub browser_native_display: bool,
    pub browser_agent_automation: bool,
    pub browser_cdp: bool,
}

impl PlatformCapabilities {
    fn current() -> Self {
        let browser_runtime_ready =
            crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
                .browser_mcp_entry()
                .is_some();
        Self::current_with_browser_runtime_ready(browser_runtime_ready)
    }

    fn current_with_browser_runtime_ready(browser_runtime_ready: bool) -> Self {
        let capabilities = crate::platform::capabilities::current();
        // The UI and the session MCP configuration must cross the same runtime
        // readiness gate. Static product support alone is insufficient when
        // Node, the wrapper, CDMCP, or WebKitWebDriver is unavailable.
        let browser_ready = capabilities.browser_native_display
            && capabilities.browser_agent_automation
            && browser_runtime_ready;
        Self {
            os: capabilities.os,
            show_megacube_site: capabilities.show_megacube_site,
            show_super_permission_settings: capabilities.show_super_permission_settings,
            uses_bundled_dependency_installer: capabilities.uses_bundled_dependency_installer,
            uses_homebrew_dependency_installer: capabilities.uses_homebrew_dependency_installer,
            task_completion_notifications_default: capabilities
                .task_completion_notifications_default,
            local_vllm_supported: capabilities.local_vllm_supported,
            codex_acp_supported: capabilities.codex_acp_supported,
            browser_native_display: browser_ready,
            browser_agent_automation: browser_ready,
            browser_cdp: capabilities.browser_cdp,
        }
    }
}

/// Return semantic UI capabilities for the compiled target.
///
/// The frontend consumes these flags instead of inferring the operating
/// system from WebView user-agent strings, which differ across runtimes.
#[tauri::command]
pub fn get_platform_capabilities() -> PlatformCapabilities {
    PlatformCapabilities::current()
}

/// Return the app-owned snapshot of shell jobs for one session. Polling this
/// command does not touch Engine lifecycle or conversation state.
#[tauri::command]
pub async fn list_shell_tasks(
    session_id: String,
    pool: State<'_, EnginePool>,
) -> Result<Vec<deepseek_tui::tools::shell::ShellJobSnapshot>, String> {
    pool.list_shell_tasks(&session_id)
        .await
        .map_err(|error| format!("list_shell_tasks({session_id}): {error:#}"))
}

/// Cancel a detached or foreground-backed shell by its stable task id.
#[tauri::command]
pub async fn cancel_shell_task(
    session_id: String,
    task_id: String,
    pool: State<'_, EnginePool>,
) -> Result<deepseek_tui::tools::shell::ShellResult, String> {
    pool.cancel_shell_task(&session_id, &task_id)
        .await
        .map_err(|error| format!("cancel_shell_task({session_id}, {task_id}): {error:#}"))
}

#[cfg(test)]
mod platform_capability_tests {
    use super::*;

    #[test]
    fn semantic_capabilities_match_the_compiled_target() {
        let capabilities = get_platform_capabilities();
        let expected = crate::platform::capabilities::current();
        assert_eq!(capabilities.os, expected.os);
        assert_eq!(
            capabilities.uses_bundled_dependency_installer,
            expected.uses_bundled_dependency_installer
        );
        assert_eq!(
            capabilities.uses_homebrew_dependency_installer,
            expected.uses_homebrew_dependency_installer
        );
        assert_eq!(
            capabilities.show_super_permission_settings,
            expected.show_super_permission_settings
        );
        assert_eq!(
            capabilities.task_completion_notifications_default,
            expected.task_completion_notifications_default
        );
        assert_eq!(
            capabilities.codex_acp_supported,
            expected.codex_acp_supported
        );
        assert_eq!(
            capabilities.browser_native_display, capabilities.browser_agent_automation,
            "UI display and Agent MCP must cross the runtime gate atomically"
        );
        assert!(!capabilities.browser_native_display || expected.browser_native_display);
        assert!(!capabilities.browser_agent_automation || expected.browser_agent_automation);
        assert_eq!(capabilities.browser_cdp, expected.browser_cdp);
    }

    #[test]
    fn browser_runtime_readiness_closes_both_public_capabilities() {
        let unavailable = PlatformCapabilities::current_with_browser_runtime_ready(false);
        assert!(!unavailable.browser_native_display);
        assert!(!unavailable.browser_agent_automation);

        let ready = PlatformCapabilities::current_with_browser_runtime_ready(true);
        let expected = crate::platform::capabilities::current();
        assert_eq!(
            ready.browser_native_display,
            expected.browser_native_display
        );
        assert_eq!(
            ready.browser_agent_automation,
            expected.browser_agent_automation
        );
    }
}
