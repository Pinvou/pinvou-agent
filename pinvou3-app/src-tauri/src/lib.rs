//! pinvou3-app Tauri 后端入口（Week 1 骨架）。
//!
//! 启动流程：
//!  1. 注册 `chat` 命令（前端 invoke 入口）
//!  2. setup 钩子里异步 spawn DeepSeek-TUI Engine + 启动事件转发 task
//!  3. 把 AppEngine 放进 Tauri State，命令通过 `State<AppEngine>` 拿
//!
//! Engine 事件（MessageDelta / ToolCallStarted / ToolCallComplete / TurnComplete）
//! 由 `engine::spawn_event_forwarder` 转译成 Tauri 事件推到前端。

// bridge + engine 公开给 tests/l1_dialog_harness.rs 用 (boot_with_workspace /
// spawn_headless 是测试入口)。其余模块保持 private,仅 Tauri 内部使用。
pub mod bridge;
mod commands;
// L1 harness 的附件 e2e 要走「真实 ingest → 注入分流 → 真 vLLM」全链路:
// 暴露注入收口函数 + file_ingest。
pub use commands::build_message_with_attachments;
pub mod engine;
pub mod engine_pool;
pub mod file_ingest;
mod file_watcher;
mod monitor;
pub mod personas;
mod pinvou_review;
pub mod super_permission;
mod updater;

use tauri::Manager;

use crate::bridge::sessions::SessionStore;
use crate::engine_pool::EnginePool;
use crate::monitor::MonitorState;

/// 为 release 安装包（.deb 双击启动场景）注入 run-dev.sh 里集中处理的运行时 env。
/// dev 启动走 run-dev.sh 已经 export 过的不会被覆盖（var_os().is_none() 守门）。
fn ensure_release_env() {
    use std::env;
    const DEFAULTS: &[(&str, &str)] = &[
        // —— vLLM 后端：BASE_URL/MODEL/API_KEY 已在 bridge/mod.rs 有默认常量，
        // 这里只补 run-dev.sh 额外注入但 Rust 没默认的 ——
        ("DEEPSEEK_PROVIDER", "vllm"),
        ("DEEPSEEK_REASONING_EFFORT", "off"),
        ("DEEPSEEK_ALLOW_INSECURE_HTTP", "1"),
        ("DEEPSEEK_FORCE_HTTP1", "1"),
        ("DEEPSEEK_MAX_OUTPUT_TOKENS", "24576"),
        ("DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS", "240"),
        // —— webkit2gtk / fcitx 兼容（Wayland 下 IM 协议挂、合成模式异常）——
        ("GDK_BACKEND", "x11"),
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
        ("GTK_IM_MODULE", "fcitx"),
        ("QT_IM_MODULE", "fcitx"),
        ("XMODIFIERS", "@im=fcitx"),
    ];
    for (k, v) in DEFAULTS {
        if env::var_os(k).is_none() {
            env::set_var(k, v);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    ensure_release_env();
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

            // 多 session 并发:存 EnginePool(lazy spawn,首条消息才为该 session 起 engine)。
            // boot bridge 在 pool::new 里做一次(写盘 / 设 env 只能一次)。
            let handle = app.handle().clone();
            let store_for_engine = session_store.unwrap_or_else(|| {
                // store boot 失败时退化用一份临时 store（让 engine 至少能起来）；
                // 实际使用 session 相关命令会失败,但聊天能跑
                SessionStore::boot().expect("session store boot fallback")
            });
            match EnginePool::new(handle.clone(), store_for_engine) {
                Ok(pool) => {
                    handle.manage(pool);
                    eprintln!("[pinvou3-app] engine pool ready (lazy spawn per session)");
                }
                Err(e) => {
                    eprintln!("[pinvou3-app] failed to init engine pool: {e:?}");
                }
            }

            // Monitor 按需采样：state 只持有 session_uptime，sample 由前端调
            // get_monitor_snapshot 时触发（监控页面 1s interval，离开页面停）。
            let monitor_state = MonitorState::new();
            app.handle().manage(monitor_state);

            // 工作流 Phase 可视化:skill 绑定挂在 SessionStore.mode_state 上,
            // per-session 隔离(start_skill_session 命令负责新建 session + bind)。
            // 不再需要全局 ActiveSkillStore。

            // File watcher: 监听 ~/.pinvou3/sessions/ 树,新文件 emit artifact:disk
            file_watcher::spawn(app.handle().clone(), bridge::paths::sessions_root());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat,
            commands::get_settings,
            commands::get_effective_model_config,
            commands::update_settings,
            commands::save_settings_and_restart,
            commands::clear_session,
            commands::get_monitor_snapshot,
            commands::get_backend_status,
            commands::discover_local_vllm,
            commands::list_sessions,
            commands::create_session,
            commands::load_session,
            commands::delete_session,
            commands::rename_session,
            commands::get_active_session,
            commands::save_session_messages,
            commands::save_session_artifacts,
            commands::list_workspace_files,
            commands::cancel_generation,
            commands::edit_last_turn,
            commands::read_artifact_text,
            commands::artifact_info,
            commands::render_artifact_visual,
            commands::open_in_system,
            commands::open_containing_folder,
            commands::open_artifact_window,
            commands::open_external_url,
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
            commands::summon_pinvou,
            commands::save_session_pinvou_reviews,
            commands::get_session_pinvou_reviews,
            commands::get_super_permission_status,
            commands::set_super_permission,
            commands::list_personas,
            commands::read_persona_body,
            commands::equip_persona,
            commands::unequip_persona,
            commands::get_active_persona,
            commands::create_persona,
            commands::update_persona,
            commands::delete_persona,
            commands::save_session_persona_events,
            commands::get_session_persona_events,
            updater::check_for_update,
            updater::download_update,
            updater::install_update,
            updater::restart_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod blocklist_contract {
    use deepseek_tui::tools::pinvou3_blocklist::{is_pinvou3_hidden, PINVOU3_HIDDEN_TOOLS};

    /// L2-4: pinvou3 L1.5 blocklist 关键不变量——防止上游 rebase 或重构时
    /// 误把整块隐藏清单删掉/改名，导致 LLM schema 重新膨胀。
    #[test]
    fn pinvou3_blocklist_hides_state_tools() {
        // 数量下限——fork 维护时一旦掉到 60 以下就要 review 是否漏砍
        assert!(
            PINVOU3_HIDDEN_TOOLS.len() >= 60,
            "blocklist 数量 {} < 60,可能整块被误删",
            PINVOU3_HIDDEN_TOOLS.len()
        );

        // 类别代表性工具必须在内（每个类别至少一个 sentinel，整类被漏砍
        // 立刻 fail）
        for sentinel in [
            "task_create",          // durable task
            "agent_open",           // subagent
            "rlm_eval",             // RLM
            "pr_attempt_record",    // PR 跟踪
            "create_goal",          // goal 状态管理
            "git_log",              // git 类
            "apply_patch",          // patch/fim
            "pandoc_convert",       // 附件预处理（移到 bridge）
            "todo_write",           // legacy todo alias
            "exec_shell_cancel",    // 异步 shell 变体
            "automation_create",    // automation 持久化
            "github_issue_context", // github 集成
            "web.run",              // 旧 web_run
        ] {
            assert!(
                is_pinvou3_hidden(sentinel),
                "类别代表工具 {sentinel} 应该被隐藏,但不在 blocklist"
            );
        }

        // 核心工具必须可见（误把 read_file 砍了 = AI 啥都干不了）
        for core in [
            "read_file",
            "write_file",
            "append_file",
            "edit_file",
            "exec_shell",
            "web_search",
            "checklist_write",
            "update_plan",
            "list_dir",
            "request_user_input",
            "exec_shell_wait",
            "git_status",
            "git_diff",
            "diagnostics",
            "revert_turn",
        ] {
            assert!(!is_pinvou3_hidden(core), "核心工具 {core} 不应该被隐藏");
        }
    }
}
