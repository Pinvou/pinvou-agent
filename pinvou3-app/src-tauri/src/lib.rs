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
mod audit;
pub mod bridge;
mod commands;
pub mod credential_store;
mod detach;
pub mod feedback;
// L1 harness 的附件 e2e 要走「真实 ingest → 注入分流 → 真 vLLM」全链路:
// 暴露注入收口函数 + file_ingest。
pub use commands::build_message_with_attachments;
// CLI 连接器公共管道(飞书 / 企微共享:起进程 / 扫码 / 事件 / 取消)。
mod connector_cli;
mod eip;
pub mod engine;
pub mod engine_pool;
mod feishu;
pub mod file_ingest;
mod file_watcher;
mod harness;
mod knowledge;
mod local_vllm_setup;
pub mod memory;
mod monitor;
mod notifications;
mod os;
pub mod personas;
mod pinvou_review;
mod process;
pub mod super_permission;
mod timing;
mod updater;
mod voice_asr;
mod wecom;
mod workflow_migrate;
pub mod workflow_registry;
mod workflow_runs;
mod zhidao;

use tauri::Manager;

use crate::bridge::sessions::SessionStore;
use crate::engine_pool::EnginePool;
use crate::monitor::MonitorState;

/// 把三省六部「网页类」预置模板 seed 到 `~/.pinvou3/web-template`（工部提示词硬编码此路径,
/// 要在副本里 `npm run build` 写盘,而随 deb 的 resource_dir 是只读安装目录,故首次启动复制一份)。
/// 已就位则跳过；用「临时目录 + 原子 rename」防半截复制留下残缺模板。失败只警告——网页类差事
/// 不可用,但不连累其余工作流。
fn seed_web_template(src: Option<std::path::PathBuf>) {
    let dst = crate::bridge::paths::web_template_dir();
    if dst.join("package.json").exists() {
        return; // 已就位
    }
    let Some(src) = src else {
        eprintln!(
            "[pinvou3-app] web-template 源缺失(resource_dir / PINVOU3_WEB_TEMPLATE_DIR 都没找到),网页类差事不可用"
        );
        return;
    };
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = dst.with_file_name("web-template.seeding");
    let _ = std::fs::remove_dir_all(&tmp); // 清上次中断的残留
    match copy_dir_all(&src, &tmp).and_then(|()| std::fs::rename(&tmp, &dst)) {
        Ok(()) => eprintln!("[pinvou3-app] web-template seeded -> {}", dst.display()),
        Err(e) => {
            eprintln!("[pinvou3-app] web-template seed 失败: {e}");
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
}

/// 递归复制目录,保留 symlink(node_modules/.bin/* 是相对 symlink,原样重建才不悬空)。
fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_symlink() {
            let target = std::fs::read_link(&from)?;
            #[cfg(unix)]
            {
                // 已存在的目标(重试场景)先删,symlink 才能重建
                let _ = std::fs::remove_file(&to);
                std::os::unix::fs::symlink(&target, &to)?;
            }
            #[cfg(not(unix))]
            {
                let _ = target;
                std::fs::copy(&from, &to)?;
            }
        } else if ft.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 为 release 安装包（.deb 双击启动场景）注入 run-dev.sh 里集中处理的运行时 env。
/// dev 启动走 run-dev.sh 已经 export 过的不会被覆盖（var_os().is_none() 守门）。
fn ensure_release_env() {
    use std::env;
    // Windows 没有 Unix 的 HOME 环境变量,但本 app 大量代码(paths.rs::user_home_dir /
    // file_ingest / bridge)用 std::env::var("HOME") 解析 `~/.pinvou3` 路径树。HOME 缺失时
    // user_home_dir() 退回 "/tmp" → 在 Windows 上拼出非法路径 `/tmp\.pinvou3\sessions`,
    // SessionStore::boot 直接 panic 闪退。这里在启动最早期(单线程,Tauri builder 之前)把
    // HOME 补成 USERPROFILE,一处设置让所有 HOME 读取点在 Windows 生效。
    #[cfg(windows)]
    if env::var_os("HOME").is_none() {
        if let Some(profile) = env::var_os("USERPROFILE") {
            env::set_var("HOME", profile);
        }
    }
    const DEFAULTS: &[(&str, &str)] = &[
        // —— vLLM 后端：BASE_URL/MODEL/API_KEY 已在 bridge/mod.rs 有默认常量，
        // 这里只补 run-dev.sh 额外注入但 Rust 没默认的 ——
        // ⚠️ 不再注入 DEEPSEEK_PROVIDER：它会被 bridge.provider() 当成 env 覆盖
        //   （env 优先级高于 preset），在「添加模型」多 provider 方案下钉死路由——
        //   切到 kimi/openai/qwen 等仍被当 vllm，且设置页误报「环境变量已锁定 provider」。
        //   provider 现由 active_model.preset 决定（LocalVllm→vllm 默认仍成立）。
        ("DEEPSEEK_REASONING_EFFORT", "off"),
        ("DEEPSEEK_ALLOW_INSECURE_HTTP", "1"),
        ("DEEPSEEK_FORCE_HTTP1", "1"),
        ("DEEPSEEK_MAX_OUTPUT_TOKENS", "24576"),
        ("DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS", "240"),
        // SSE 首响应头超时(open timeout):底座只认 env,默认 45s 是为云端调的。
        // 本地 GB10 大上下文 SubAgent 请求首 token TTFT 偶发 >45s → 误杀子 agent
        // (真机实锤:三省六部 libu~1 首发死于 45s,重派才过)。280s 与
        // ~/.deepseek config 的 subagent api_timeout=300 对齐(步级超时须更大)。
        ("DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS", "280"),
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

    // IT 内部 CLI 业务调用走 bin 里的包装脚本(`eip`/`zhidao` 等,内部注入 AGENT_*)——因为模型
    // shell 工具的环境是白名单消毒过的,AGENT_DEVICE_ID/CREDENTIALS_DIR/NON_INTERACTIVE
    // 直接继承会被过滤掉(见 eip.rs 顶注 + EIP 接入方案 §2.5)。PATH 在白名单内可继承,
    // 故把 `~/.pinvou3/bundle/skills/{eip,zhidao}/bin` 前插进程 PATH,模型即可像 lark-cli 一样
    // 直接调用。目录此刻可能尚未解包,但 PATH 含不存在目录无害。
    {
        let skills_dir = crate::bridge::paths::bundle_skills_dir();
        if let Some(old) = env::var_os("PATH") {
            let mut dirs = vec![
                skills_dir.join("eip").join("bin"),
                skills_dir.join("zhidao").join("bin"),
            ];
            if let Some(connector_bin) = crate::bridge::paths::bundle_connector_bin_dir() {
                dirs.push(connector_bin);
            }
            if let Ok(prefix) = env::var("NPM_CONFIG_PREFIX") {
                dirs.push(std::path::Path::new(&prefix).join("bin"));
            }
            if let Some(home) = env::var_os("HOME") {
                let home = std::path::Path::new(&home);
                dirs.push(home.join(".npm-global").join("bin"));
                dirs.push(home.join(".local").join("bin"));
            }
            dirs.extend(env::split_paths(&old));
            if let Ok(joined) = env::join_paths(dirs) {
                env::set_var("PATH", joined);
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    ensure_release_env();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Linux webview(webkit2gtk)默认拒绝 getUserMedia,语音输入点麦克风会被拒。
            // 给 main 窗口 webview 挂 permission-request:只放行 UserMedia(麦克风/摄像头)
            // 请求,定位/通知等其余权限仍按默认拒绝。Windows/macOS 的 WebView2/WKWebView
            // 自带系统级麦克风授权,不走这条。
            #[cfg(target_os = "linux")]
            {
                use tauri::Manager;
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.with_webview(|webview| {
                        use webkit2gtk::glib::prelude::ObjectExt;
                        use webkit2gtk::{PermissionRequestExt, WebViewExt};
                        let wv = webview.inner();
                        wv.connect_permission_request(|_wv, req| {
                            if req.type_().name() == "WebKitUserMediaPermissionRequest" {
                                req.allow();
                                true
                            } else {
                                false
                            }
                        });
                    });
                }
            }

            // run 实体化一次性迁移：必须在 SessionStore boot **之前**跑
            // （迁移会动 _skill_bindings.json 和 sessions/ 目录，boot 之后再动
            // 会跟内存态打架）。失败只警告不 panic——app 仍可用，下次 boot 续跑。
            if let Err(e) = crate::workflow_migrate::migrate_if_needed() {
                eprintln!("[pinvou3-app] workflow migrate failed (will retry next boot): {e}");
            }

            // 多对话历史 store：用 ~/.pinvou3/sessions/ 隔离 deepseek-tui 全局目录。
            // 必须先 boot 这个，engine forwarder 需要它跟踪 active session 的 mode_state
            // 以便 TurnComplete 时判定是否 emit chat:plan_ready。
            let session_store = match SessionStore::boot() {
                Ok(store) => {
                    store.load_skill_bindings();
                    store.load_session_models();
                    store.load_pinned_sessions();
                    store.load_hidden_sessions();
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
            match EnginePool::new(handle.clone(), store_for_engine.clone()) {
                Ok(pool) => {
                    handle.manage(pool);
                    eprintln!("[pinvou3-app] engine pool ready (lazy spawn per session)");
                }
                Err(e) => {
                    eprintln!("[pinvou3-app] failed to init engine pool: {e:?}");
                }
            }

            // 技能停用联动:启动时按当前被禁用连接器的 companion_skills 推给底座进程级
            // 过滤器,让(如公文 MCP 关掉时的)关联技能从首轮 prompt 起就不出现在 ## Skills。
            crate::bridge::skill_marketplace::refresh_disabled_skills();

            // Monitor 按需采样：state 只持有 session_uptime，sample 由前端调
            // get_monitor_snapshot 时触发（监控页面 1s interval，离开页面停）。
            let monitor_state = MonitorState::new();
            app.handle().manage(monitor_state);
            app.handle()
                .manage(notifications::NotificationState::default());

            // CLI 连接器连接编排状态(按连接器 id 存长驻子进程 PID + 取消标志),
            // 飞书 / 企微共用,供 *_connect_begin / *_cancel 用。
            app.handle().manage(connector_cli::ConnectorConn::default());

            // EIP 连接编排状态(SSO 登录轮询取消标志),供 eip_connect_begin/cancel 用。
            app.handle().manage(eip::EipConn::default());

            // 知道连接编排状态(SSO 登录取消标志),供 zhidao_connect_begin/cancel 用。
            app.handle().manage(zhidao::ZhidaoConn::default());

            // 工作流 Phase 可视化:skill 绑定挂在 SessionStore.mode_state 上,
            // per-session 隔离(start_skill_session 命令负责新建 session + bind)。
            // 不再需要全局 ActiveSkillStore。

            // File watcher: 监听 ~/.pinvou3/sessions/ 树,新文件 emit artifact:disk
            file_watcher::spawn(app.handle().clone(), bridge::paths::sessions_root());

            // 本地知识底座 L0:全系统元数据索引(秒搜+去重)。这里只 manage,**不自动扫**——
            // 扫描改懒触发:由前端进入文件管理页时增量扫(不进页=零扫描),不常驻 watcher/周期
            // 重扫。文件管理是低频功能,不该长期占资源。
            // embedding 模型**不再随 deb 打包**(deb 瘦 ~559MB):改按需下载到
            // ~/.pinvou3/knowledge/models/bge-m3(knowledge::model_dir)。这里把下载落点作 fallback
            // 传给服务;dev 的 env(PINVOU3_KB_EMBED_MODEL_DIR,run-dev.sh 设)优先逻辑仍由
            // embed::from_env_or_dir 内部保留。模型没装 → 加载失败 → embedder=None → 知识库走
            // 完全门控(前端 gate),不阻断启动;用户在知识库页下载后 reload_embedder 热加载。
            let kb_model_dir = knowledge::model_dir();
            // 语音识别引擎 sense-voice-main 随 deb 打包,容错同 bge-m3 的资源布局,
            // 注入给 voice_asr 作为 ~/.pinvou3/asr/ 之外的回退查找目录。
            if let Some(asr_res) = app.path().resource_dir().ok().and_then(|res| {
                [
                    res.join("asr"),
                    res.join("resources/asr"),
                    res.join("resources").join("asr"),
                ]
                .into_iter()
                .find(|d| d.join("sense-voice-main").exists())
            }) {
                voice_asr::set_bundled_engine_dir(asr_res);
            }

            match knowledge::KnowledgeService::new(
                &knowledge::default_db_path(),
                Some(&kb_model_dir),
            ) {
                Ok(svc) => {
                    app.handle().manage(svc);
                    eprintln!("[pinvou3-app] knowledge service ready");
                }
                Err(e) => eprintln!("[pinvou3-app] knowledge service init failed: {e:?}"),
            }

            // 三省六部「网页类」预置模板 seed(工部 `cp -r ~/.pinvou3/web-template ...` 的母版)。
            // dev 走 env PINVOU3_WEB_TEMPLATE_DIR(run-dev.sh 注入 ~/models/web-template);prod 从
            // 随 deb 的 resource_dir 容错三布局取(对齐上面 bge-m3 那段)。69M/2904 文件,放后台
            // 线程复制,不阻塞启动；已就位则秒跳过。
            {
                let web_tpl_src = std::env::var_os("PINVOU3_WEB_TEMPLATE_DIR")
                    .map(std::path::PathBuf::from)
                    .filter(|d| d.join("package.json").exists())
                    .or_else(|| {
                        app.path().resource_dir().ok().and_then(|res| {
                            [
                                res.join("web-template"),
                                res.join("resources/web-template"),
                                res.join("resources").join("web-template"),
                            ]
                            .into_iter()
                            .find(|d| d.join("package.json").exists())
                        })
                    });
                std::thread::spawn(move || seed_web_template(web_tpl_src));
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat,
            feishu::feishu_ensure_cli,
            feishu::feishu_status,
            feishu::feishu_connect_begin,
            feishu::feishu_cancel,
            feishu::feishu_logout,
            feishu::feishu_apply_skills,
            feishu::set_feishu_enabled,
            feishu::feishu_skills_state,
            wecom::wecom_ensure_cli,
            wecom::wecom_status,
            wecom::wecom_connect_begin,
            wecom::wecom_cancel,
            wecom::wecom_logout,
            wecom::wecom_apply_skills,
            wecom::set_wecom_enabled,
            wecom::wecom_skills_state,
            eip::eip_status,
            eip::eip_connect_begin,
            eip::eip_cancel,
            eip::eip_logout,
            zhidao::zhidao_status,
            zhidao::zhidao_connect_begin,
            zhidao::zhidao_cancel,
            zhidao::zhidao_logout,
            commands::get_settings,
            commands::submit_feedback,
            commands::get_effective_model_config,
            commands::update_settings,
            commands::save_settings_and_restart,
            commands::clear_session,
            commands::get_monitor_snapshot,
            commands::get_backend_status,
            commands::discover_local_vllm,
            local_vllm_setup::detect_local_vllm_setup,
            local_vllm_setup::bootstrap_local_vllm,
            local_vllm_setup::decline_local_vllm_setup,
            commands::list_models,
            commands::save_model,
            commands::delete_model,
            commands::set_active_model,
            commands::set_session_model,
            commands::get_session_model_id,
            commands::test_model_connection,
            commands::transcribe_voice_audio,
            voice_asr::voice_asr_status,
            voice_asr::install_voice_asr,
            commands::list_sessions,
            commands::create_session,
            commands::load_session,
            commands::delete_session,
            commands::rename_session,
            commands::set_session_pinned,
            commands::list_archived_sessions,
            commands::set_session_archived,
            commands::get_active_session,
            commands::save_session_messages,
            commands::save_session_artifacts,
            commands::list_workspace_files,
            commands::cancel_generation,
            commands::set_disabled_connectors,
            commands::get_disabled_connectors,
            commands::get_memory_profile,
            commands::update_memory_profile,
            commands::clear_memory_profile,
            commands::get_memory_overview,
            commands::list_pending_memory,
            commands::suggest_memory,
            commands::confirm_pending_memory,
            commands::ignore_pending_memory,
            commands::never_pending_memory,
            commands::list_recent_work_memory,
            commands::upsert_recent_work_memory,
            commands::archive_recent_work_memory,
            commands::delete_memory_preference,
            commands::update_memory_preference,
            commands::update_work_context_memory,
            commands::delete_work_context_memory,
            commands::update_timed_memory,
            commands::delete_timed_memory,
            commands::edit_last_turn,
            commands::read_artifact_text,
            commands::list_deliverables,
            commands::list_deliverable_index,
            commands::artifact_info,
            commands::render_artifact_visual,
            commands::read_artifact_image_b64,
            commands::read_artifact_thumbnail,
            commands::open_in_system,
            commands::open_containing_folder,
            commands::reveal_session_folder,
            commands::open_artifact_window,
            detach::open_detached_window,
            detach::begin_detach_drag,
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
            commands::read_skill_body,
            commands::list_skills_v2,
            commands::read_skill_demo,
            commands::start_skill_session,
            commands::unbind_session_skill,
            commands::list_workflows,
            commands::start_workflow,
            commands::kick_workflow,
            commands::retry_workflow_role,
            commands::get_role_prompt,
            commands::get_role_outputs,
            commands::get_role_logs,
            commands::get_gate_report,
            commands::save_project_config,
            commands::save_agent_overrides,
            commands::cancel_workflow_role,
            commands::approve_workflow_gate,
            commands::reject_workflow_gate,
            commands::get_workflow_state,
            commands::find_resumable_run,
            commands::get_session_active_skill,
            commands::list_session_skill_bindings,
            commands::submit_user_input,
            commands::add_run_materials,
            commands::cancel_user_input,
            commands::restart_engine,
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
            updater::get_app_version,
            updater::check_for_update,
            updater::download_update,
            updater::install_update,
            updater::restart_app,
            updater::cancel_download,
            updater::report_pending_update_result,
            file_ingest::check_dependencies,
            file_ingest::install_dependencies,
            commands::list_marketplace_tools,
            commands::install_marketplace_tool,
            commands::uninstall_marketplace_tool,
            commands::detect_obsidian,
            knowledge::kb_start_scan,
            knowledge::kb_scan_status,
            knowledge::kb_cancel_scan,
            knowledge::kb_search,
            knowledge::kb_stats,
            knowledge::kb_type_counts,
            knowledge::kb_collection_list,
            knowledge::kb_collection_create,
            knowledge::kb_collection_update,
            knowledge::kb_collection_delete,
            knowledge::kb_collection_add_sources,
            knowledge::kb_index_status,
            knowledge::kb_index_cancel,
            knowledge::kb_documents,
            knowledge::kb_remove_document,
            knowledge::kb_embed_info,
            knowledge::model_download::kb_model_status,
            knowledge::model_download::kb_model_download,
            knowledge::model_download::kb_model_cancel,
            commands::session_mount_collection,
            commands::session_unmount_collection,
            commands::session_mounted_collection,
            commands::list_marketplace_skills,
            commands::install_marketplace_skill,
            commands::import_skill_package,
            commands::uninstall_marketplace_skill,
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
            "tool_agent",           // subagent spawn 工具隐藏(spawn 单一走 agent_open)
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
            // git_status/git_diff/diagnostics 已于 2026-07-03 纯办公定位决策砍入 blocklist（放弃代码辅助），不再要求可见
            "revert_turn",
            "agent_open",  // subagent spawn(单一 spawn 入口)
            "agent_eval",  // subagent 收结果
            "agent_close", // subagent 释放 session
            "kb_search",   // Agentic RAG: app 注入的本地知识检索工具,必须对模型可见
        ] {
            assert!(!is_pinvou3_hidden(core), "核心工具 {core} 不应该被隐藏");
        }
    }
}

#[cfg(test)]
mod web_template_seed {
    use std::fs;
    use std::path::PathBuf;

    fn tmp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pinvou3-wt-{tag}-{}", std::process::id()))
    }

    /// copy_dir_all 的关键不变量:递归复制文件 + **保留 symlink**。
    /// web-template 的 node_modules/.bin/* 全是相对 symlink,被解引用成普通文件会撑爆体积
    /// 且破坏 npm 可执行入口 → `npm run build` 失败。
    #[test]
    #[cfg(unix)]
    fn copy_dir_all_preserves_files_and_symlinks() {
        let root = tmp_root("copy");
        let _ = fs::remove_dir_all(&root);
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("sub/b.txt"), b"world").unwrap();
        std::os::unix::fs::symlink("sub/b.txt", src.join("link")).unwrap();

        super::copy_dir_all(&src, &dst).unwrap();

        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(dst.join("sub/b.txt")).unwrap(), b"world");
        let meta = fs::symlink_metadata(dst.join("link")).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "symlink 必须保留为 symlink,不能解引用"
        );
        assert_eq!(
            fs::read_link(dst.join("link")).unwrap(),
            PathBuf::from("sub/b.txt"),
            "symlink target 不变"
        );
        assert_eq!(
            fs::read(dst.join("link")).unwrap(),
            b"world",
            "跟随 symlink 仍读到内容"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn web_template_dir_named_web_template() {
        assert!(crate::bridge::paths::web_template_dir().ends_with("web-template"));
    }
}
