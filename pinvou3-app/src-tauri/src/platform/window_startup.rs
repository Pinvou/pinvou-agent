#[cfg(target_os = "linux")]
use gtk::prelude::*;
#[cfg(target_os = "linux")]
use tauri::Manager;

#[cfg(any(target_os = "linux", test))]
const HIDDEN_MAIN_WINDOW_FALLBACK_SECS: u64 = 8;

/// Reveal only the Linux main window that the platform overlay creates hidden.
/// The cfg gate keeps other platforms behaviorally unchanged.
pub fn reveal_startup_window(window: tauri::WebviewWindow) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        if window.label() != "main" {
            return Ok(false);
        }
        if window.is_visible().map_err(|error| error.to_string())? {
            return Ok(false);
        }

        let main_window = window.clone();
        window
            .run_on_main_thread(move || match main_window.gtk_window() {
                Ok(gtk_window) => {
                    // 同一 GTK 主循环任务内完成映射、取消 iconic 状态和前台呈现。
                    // 避免 release 启动较快时，异步 set_focus 早于窗口 map 而被 Mutter 丢弃。
                    gtk_window.show_all();
                    gtk_window.deiconify();
                    gtk_window.present();
                    crate::platform::startup::mark("tauri:startup_window_revealed");
                }
                Err(error) => crate::platform::startup::mark_with_detail(
                    "rust",
                    "tauri:startup_window_reveal_error",
                    &error.to_string(),
                ),
            })
            .map_err(|error| error.to_string())?;
        return Ok(true);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = window;
        Ok(false)
    }
}

/// Linux 通过 overlay 隐藏创建主窗口，正常由 React 首次提交负责显示。
/// 若前端模块加载失败，超时后仍显示窗口，避免进程运行但用户完全无从诊断。
pub(crate) fn arm_hidden_main_window_fallback(app: &tauri::AppHandle) {
    #[cfg(target_os = "linux")]
    {
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        if window.is_visible().unwrap_or(true) {
            return;
        }
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(
                HIDDEN_MAIN_WINDOW_FALLBACK_SECS,
            ))
            .await;
            if !matches!(window.is_visible(), Ok(false)) {
                return;
            }
            match reveal_startup_window(window) {
                Ok(true) => crate::platform::startup::mark("tauri:main_window_fallback_revealed"),
                Ok(false) => {}
                Err(error) => crate::platform::startup::mark_with_detail(
                    "rust",
                    "tauri:main_window_fallback_error",
                    &error,
                ),
            }
        });
    }

    #[cfg(not(target_os = "linux"))]
    let _ = app;
}

#[cfg(test)]
mod tests {
    use super::HIDDEN_MAIN_WINDOW_FALLBACK_SECS;

    #[test]
    fn fallback_leaves_time_for_a_cold_frontend_build() {
        assert_eq!(HIDDEN_MAIN_WINDOW_FALLBACK_SECS, 8);
    }
}
