//! pinvou3-app Tauri 后端入口（Week 1 骨架）。
//!
//! 启动流程：
//!  1. 注册 `chat` 命令（前端 invoke 入口）
//!  2. setup 钩子里异步 spawn DeepSeek-TUI Engine + 启动事件转发 task
//!  3. 把 AppEngine 放进 Tauri State，命令通过 `State<AppEngine>` 拿
//!
//! Engine 事件（MessageDelta / ToolCallStarted / ToolCallComplete / TurnComplete）
//! 由 `engine::spawn_event_forwarder` 转译成 Tauri 事件推到前端。

mod commands;
mod engine;

use tauri::Manager;

use crate::engine::AppEngine;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // 异步 spawn engine：Tauri setup 是同步的，需要走 async_runtime
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                match AppEngine::spawn(handle.clone()).await {
                    Ok(eng) => {
                        handle.manage(eng);
                        eprintln!("[pinvou3-app] engine ready");
                    }
                    Err(e) => {
                        eprintln!("[pinvou3-app] failed to spawn engine: {e:?}");
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::chat])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
