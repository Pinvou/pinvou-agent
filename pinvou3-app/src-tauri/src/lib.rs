//! pinvou3-app Tauri 后端入口（Week 1 骨架）。
//!
//! 启动流程：
//!  1. 注册 `chat` 命令（前端 invoke 入口）
//!  2. setup 钩子里异步 spawn DeepSeek-TUI Engine + 启动事件转发 task
//!  3. 把 AppEngine 放进 Tauri State，命令通过 `State<AppEngine>` 拿
//!
//! Engine 事件（MessageDelta / ToolCallStarted / ToolCallComplete / TurnComplete）
//! 由 `engine::spawn_event_forwarder` 转译成 Tauri 事件推到前端。

mod bridge;
mod commands;
mod engine;
mod file_ingest;
mod file_watcher;
mod monitor;

use std::time::Duration;

use tauri::Manager;

use crate::bridge::sessions::SessionStore;
use crate::engine::AppEngine;
use crate::monitor::MonitorState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // 多对话历史 store：用 ~/.pinvou3/sessions/ 隔离 deepseek-tui 全局目录。
            // 必须先 boot 这个，engine forwarder 需要它跟踪 active session 的 mode_state
            // 以便 TurnComplete 时判定是否 emit chat:plan_ready。
            let session_store = match SessionStore::boot() {
                Ok(store) => {
                    eprintln!("[pinvou3-app] session store ready");
                    Some(store)
                }
                Err(e) => {
                    eprintln!("[pinvou3-app] session store boot failed: {e:?}");
                    None
                }
            };
            if let Some(store) = session_store.clone() {
                app.handle().manage(store);
            }

            // 异步 spawn engine：Tauri setup 是同步的，需要走 async_runtime
            let handle = app.handle().clone();
            let store_for_engine = session_store.unwrap_or_else(|| {
                // store boot 失败时退化用一份临时 store（让 engine 至少能起来）；
                // 实际使用 session 相关命令会失败,但聊天能跑
                SessionStore::boot().expect("session store boot fallback")
            });
            tauri::async_runtime::block_on(async move {
                match AppEngine::spawn(handle.clone(), store_for_engine).await {
                    Ok(eng) => {
                        handle.manage(eng);
                        eprintln!("[pinvou3-app] engine ready");
                    }
                    Err(e) => {
                        eprintln!("[pinvou3-app] failed to spawn engine: {e:?}");
                    }
                }
            });

            // Monitor 后台采样：5s 一次，缓存在 MonitorState 里
            let monitor_state = MonitorState::new();
            monitor::spawn_sampler(monitor_state.clone(), Duration::from_secs(5));
            app.handle().manage(monitor_state);
            eprintln!("[pinvou3-app] monitor sampler started (5s interval)");

            // File watcher: 监听 ~/.pinvou3/sessions/ 树,新文件 emit artifact:disk
            file_watcher::spawn(
                app.handle().clone(),
                bridge::paths::sessions_root(),
            );

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat,
            commands::get_settings,
            commands::update_settings,
            commands::clear_session,
            commands::get_monitor_snapshot,
            commands::get_backend_status,
            commands::list_sessions,
            commands::create_session,
            commands::load_session,
            commands::delete_session,
            commands::rename_session,
            commands::get_active_session,
            commands::save_session_messages,
            commands::save_session_artifacts,
            commands::cancel_generation,
            commands::edit_last_turn,
            commands::read_artifact_text,
            commands::artifact_info,
            commands::open_in_system,
            commands::open_containing_folder,
            commands::open_artifact_window,
            commands::ingest_file,
            commands::detect_system_tools,
            commands::save_paste_image,
            commands::compact_now,
            commands::get_mode_state,
            commands::set_plan_mode_next,
            commands::exit_plan_to_yolo,
            commands::accept_plan,
            commands::discard_plan,
            commands::submit_user_input,
            commands::cancel_user_input,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
