#[cfg(target_os = "linux")]
use gtk::prelude::*;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use tauri::Manager;

#[cfg(any(target_os = "linux", test))]
const HIDDEN_MAIN_WINDOW_FALLBACK_SECS: u64 = 8;
#[cfg(target_os = "linux")]
static MAIN_WINDOW_STARTUP_READY: AtomicBool = AtomicBool::new(false);

#[cfg(any(target_os = "linux", test))]
fn should_defer_main_window_activation(startup_ready: bool, is_visible: bool) -> bool {
    !startup_ready && !is_visible
}

/// 8 秒兜底是否应真正 reveal 主窗口：仅当等待期满后窗口仍不可见。
/// 调用方需先把 `is_visible()` 的 Err 折叠为"可见"，即"不明确隐藏就视为可见"。
#[cfg(any(target_os = "linux", test))]
fn should_fallback_reveal(visible_after_wait: bool) -> bool {
    !visible_after_wait
}

#[cfg(target_os = "linux")]
fn present_linux_main_window(
    window: tauri::WebviewWindow,
    success_mark: &'static str,
    error_mark: &'static str,
) -> Result<(), String> {
    let main_window = window.clone();
    window
        .run_on_main_thread(move || match main_window.gtk_window() {
            Ok(gtk_window) => {
                // 同一 GTK 主循环任务内完成映射、取消 iconic 状态和前台呈现。
                // 避免 release 启动较快时，异步 set_focus 早于窗口 map 而被 Mutter 丢弃。
                gtk_window.show_all();
                gtk_window.deiconify();
                gtk_window.present();
                crate::platform::startup::mark(success_mark);
            }
            Err(error) => {
                crate::platform::startup::mark_with_detail("rust", error_mark, &error.to_string())
            }
        })
        .map_err(|error| error.to_string())
}

/// Reveal only the Linux main window that the platform overlay creates hidden.
/// The cfg gate keeps other platforms behaviorally unchanged.
pub fn reveal_startup_window(window: tauri::WebviewWindow) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        if window.label() != "main" {
            return Ok(false);
        }
        MAIN_WINDOW_STARTUP_READY.store(true, Ordering::Release);
        if window.is_visible().map_err(|error| error.to_string())? {
            return Ok(false);
        }

        present_linux_main_window(
            window,
            "tauri:startup_window_revealed",
            "tauri:startup_window_reveal_error",
        )?;
        return Ok(true);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = window;
        Ok(false)
    }
}

/// Bring an already-running main window forward without bypassing the Linux
/// cold-start readiness gate used by the hidden startup overlay.
pub(crate) fn activate_main_window(window: tauri::WebviewWindow) {
    #[cfg(target_os = "linux")]
    {
        let startup_ready = MAIN_WINDOW_STARTUP_READY.load(Ordering::Acquire);
        let is_visible = window.is_visible().unwrap_or(true);
        if should_defer_main_window_activation(startup_ready, is_visible) {
            // 冷启动未完成且窗口仍隐藏：defer 而非强行 present，避免把尚未稳定的
            // 输入表面暴露给第二实例用户；前端首次提交或 8 秒兜底会负责显示窗口。
            // 边界：若前端始终不 reveal，第二实例在兜底前启动将无窗口反馈（仅日志）。
            crate::platform::startup::mark("tauri:single_instance_activation_deferred");
            return;
        }
        if let Err(error) = present_linux_main_window(
            window,
            "tauri:single_instance_window_presented",
            "tauri:single_instance_window_present_error",
        ) {
            crate::platform::startup::mark_with_detail(
                "rust",
                "tauri:single_instance_window_dispatch_error",
                &error,
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
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
            MAIN_WINDOW_STARTUP_READY.store(true, Ordering::Release);
            return;
        }
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(
                HIDDEN_MAIN_WINDOW_FALLBACK_SECS,
            ))
            .await;
            // 等待期内前端已正常 reveal（或 is_visible 出错），兜底不再介入。
            if !should_fallback_reveal(window.is_visible().unwrap_or(true)) {
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
    use super::{
        HIDDEN_MAIN_WINDOW_FALLBACK_SECS, should_defer_main_window_activation,
        should_fallback_reveal,
    };

    #[test]
    fn fallback_waits_for_a_cold_frontend_build_then_reveals_only_if_still_hidden() {
        // 等待窗口须给冷前端构建留出时间（常量值本身是契约的一部分）。
        assert_eq!(HIDDEN_MAIN_WINDOW_FALLBACK_SECS, 8);
        // 仅当等待期满后仍隐藏才 reveal；已可见（前端已正常显示）或状态未知则不介入。
        assert!(should_fallback_reveal(false));
        assert!(!should_fallback_reveal(true));
    }

    #[test]
    fn second_instance_waits_only_while_cold_start_is_hidden() {
        assert!(should_defer_main_window_activation(false, false));
        assert!(!should_defer_main_window_activation(false, true));
        assert!(!should_defer_main_window_activation(true, false));
        // 已就绪且窗口可见：第二实例激活直接 present，不 defer。
        assert!(!should_defer_main_window_activation(true, true));
    }
}
