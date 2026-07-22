// ===================== 阶段 C: 取消生成 + 编辑/重发 =====================

/// 取消当前生成（生成中按⏹️停止按钮）。
/// engine 立即 cancel_token.cancel()，turn loop 跳出后会发 TurnComplete 事件，
/// 前端通过 chat:done 解锁 busy 状态；若 engine 已不存在但池仍记录活动 turn，
/// EnginePool 会补发 Interrupted 终态。空闲会话取消保持 no-op。
#[tauri::command]
pub async fn cancel_generation(
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    // 多 session:取消指定 session(前端传 session_id);兼容旧前端回退 active。
    if let Some(sid) = session_id.or_else(|| store.active_id()) {
        log::info!("[pinvou3][chat] cancel requested sid={}", sid);
        pool.cancel(&sid).await;
        log::info!("[pinvou3][chat] cancel command completed sid={}", sid);
    } else {
        log::warn!("[pinvou3][chat] cancel requested but no session id is available");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilities {
    pub os: &'static str,
    pub show_megacube_site: bool,
    pub show_super_permission_settings: bool,
    pub uses_bundled_dependency_installer: bool,
    pub task_completion_notifications_default: bool,
}

impl PlatformCapabilities {
    fn current() -> Self {
        Self {
            os: std::env::consts::OS,
            show_megacube_site: cfg!(target_os = "linux"),
            show_super_permission_settings: cfg!(target_os = "linux"),
            uses_bundled_dependency_installer: cfg!(target_os = "windows"),
            task_completion_notifications_default: !cfg!(target_os = "linux"),
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
        assert_eq!(capabilities.os, std::env::consts::OS);
        assert_eq!(
            capabilities.uses_bundled_dependency_installer,
            cfg!(target_os = "windows")
        );
        assert_eq!(
            capabilities.show_super_permission_settings,
            cfg!(target_os = "linux")
        );
        assert_eq!(
            capabilities.task_completion_notifications_default,
            !cfg!(target_os = "linux")
        );
    }
}
