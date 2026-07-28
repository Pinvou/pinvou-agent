//! pinvou3-app Tauri 后端入口（Week 1 骨架）。
//!
//! 启动流程：
//!  1. 注册 `chat` 命令（前端 invoke 入口）
//!  2. setup 钩子里异步 spawn CodeWhale Engine + 启动事件转发 task
//!  3. 把 AppEngine 放进 Tauri State，命令通过 `State<AppEngine>` 拿
//!
//! Engine 事件（MessageDelta / ToolCallStarted / ToolCallComplete / TurnComplete）
//! 由 `engine::spawn_event_forwarder` 转译成 Tauri 事件推到前端。

mod app;
mod core;
pub mod features;
pub mod platform;
// L1 harness 的附件 e2e 要走「真实 ingest → 注入分流 → 真 vLLM」全链路:
// 暴露注入收口函数 + file_ingest。
pub use app::commands::attachments::build_message_with_attachments;

use tauri::Manager;

use crate::app::commands;
use crate::features::{
    assistant::{engine_pool::EnginePool, platform::bridge},
    connectors::connector_cli,
    files::file_watcher,
    knowledge,
    monitor::MonitorState,
    pet::{pet_window, selected_pet},
    remote_control::RemoteControlManager,
    scheduled::tasks as scheduled_tasks,
    sessions::SessionStore,
};
use crate::platform::{notifications, startup};

/// 把三省六部「网页类」预置模板 seed 到 `~/.pinvou3/web-template`（工部提示词硬编码此路径,
/// 要在副本里 `npm run build` 写盘,而随 deb 的 resource_dir 是只读安装目录,故首次启动复制一份)。
/// 已就位则跳过；用「临时目录 + 原子 rename」防半截复制留下残缺模板。失败只警告——网页类差事
/// 不可用,但不连累其余工作流。
fn seed_web_template(src: Option<std::path::PathBuf>) {
    let dst = crate::platform::paths::web_template_dir();
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

const RELEASE_ENV_DEFAULTS: &[(&str, &str)] = &[
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
    // 与 CodeWhale 的 stream_chunk_timeout 默认值保持一致。
    ("DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS", "300"),
    // SSE 首响应头超时(open timeout):底座只认 env,默认 45s 是为云端调的。
    // 本地 GB10 大上下文 SubAgent 请求首 token TTFT 偶发 >45s → 误杀子 agent
    // (真机实锤:三省六部 libu~1 首发死于 45s,重派才过)。280s 与
    // ~/.deepseek config 的 subagent api_timeout=300 对齐(步级超时须更大)。
    ("DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS", "280"),
];

/// 为 release 安装包（.deb 双击启动场景）注入 run-dev.sh 里集中处理的运行时 env。
/// dev 启动走 run-dev.sh 已经 export 过的不会被覆盖（var_os().is_none() 守门）。
fn ensure_release_env() {
    use std::env;
    crate::platform::ui_cache::configure_runtime_environment();
    for (k, v) in RELEASE_ENV_DEFAULTS {
        if env::var_os(k).is_none() {
            env::set_var(k, v);
        }
    }

    {
        if let Some(old) = env::var_os("PATH") {
            let mut dirs = Vec::new();
            if let Some(connector_bin) = crate::platform::paths::bundle_connector_bin_dir() {
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
            #[cfg(target_os = "macos")]
            {
                dirs.push(std::path::PathBuf::from("/opt/homebrew/bin"));
                dirs.push(std::path::PathBuf::from("/usr/local/bin"));
                dirs.push(std::path::PathBuf::from(
                    "/Applications/LibreOffice.app/Contents/MacOS",
                ));
            }
            dirs.extend(env::split_paths(&old));
            if let Ok(joined) = env::join_paths(dirs) {
                env::set_var("PATH", joined);
            }
        }
    }
}

/// 进程级选定并安装 rustls 的 CryptoProvider(aws-lc-rs)。
///
/// 幂等:`install_default` 返回 `Err` 表示已装过(本次或之前的 reqwest/tungstenite 调用),
/// 用 `drop` 吞掉即可,不会二次注册。装在 `run()` 最前,保证后续 reqwest / relay 的
/// `connect_async` 都用已确定的 provider,避开「aws-lc-rs + ring 同时启用时无参
/// ClientConfig::builder().expect() panic」(见 Cargo.toml rustls 注释)。
fn install_rustls_provider() {
    drop(rustls::crypto::aws_lc_rs::default_provider().install_default());
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 必须最先执行:进程级选定 rustls CryptoProvider。
    // 见 Cargo.toml 的 rustls/reqwest 注释——reqwest 0.13 自带 aws-lc-rs 但只「借用」
    // provider,不写入默认槽;而 oauth2(经 reqwest 0.12)把 rustls 0.23 的 `ring` feature
    // 也拉进图,使 rustls 同时启用 aws-lc-rs + ring 两个 provider。tokio-tungstenite 0.30
    // 的 TLS 连接器走无参 ClientConfig::builder(),两 provider 共存时会 panic。此处选定
    // aws-lc-rs(FIPS 可候选、性能优于 ring),在任何 TLS 连接之前装好。
    install_rustls_provider();
    ensure_release_env();
    startup::init();
    startup::mark("environment:ready");
    // 必须早于 Tauri Builder/WebView 创建：避免升级后 WebKit 复用旧 index.html，
    // 却在新包内找不到旧 CSS，退化成裸 HTML 页面。
    crate::platform::ui_cache::migrate_before_webview();
    let initial_navigation_reported =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let main_navigation_origin = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let builder = tauri::Builder::default()
        // These no-op probe plugins bracket Tauri's own plugin initialization.
        // The main window is created by Tauri before the application setup hook,
        // so their lifecycle hooks expose time that was previously one opaque gap.
        .plugin({
            let initial_navigation_reported = initial_navigation_reported.clone();
            let main_navigation_origin = main_navigation_origin.clone();
            tauri::plugin::Builder::<_, ()>::new("startup-probe-runtime")
                .setup(|_app, _api| {
                    startup::mark("tauri:runtime_created");
                    Ok(())
                })
                .on_window_ready(|window| {
                    if window.label() == "main" {
                        startup::mark("tauri:main_window_ready");
                    }
                })
                .on_webview_ready(|webview| {
                    if webview.label() == "main" {
                        startup::mark("tauri:main_webview_ready");
                    }
                })
                .on_navigation(move |webview, url| {
                    use std::sync::atomic::Ordering;

                    if webview.label() != "main" {
                        return true;
                    }
                    let origin = format!(
                        "{}://{}:{}",
                        url.scheme(),
                        url.host_str().unwrap_or_default(),
                        url.port_or_known_default()
                            .map_or_else(|| "-".to_string(), |port| port.to_string())
                    );
                    if !initial_navigation_reported.swap(true, Ordering::Relaxed) {
                        startup::mark_with_detail(
                            "rust",
                            "tauri:main_navigation",
                            &format!("scheme={}", url.scheme()),
                        );
                    }
                    let mut initial_origin = main_navigation_origin
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match initial_origin.as_ref() {
                        None => {
                            *initial_origin = Some(origin);
                            true
                        }
                        Some(allowed) if allowed == &origin => true,
                        Some(_) => {
                            startup::mark_with_detail(
                                "rust",
                                "tauri:main_navigation_blocked",
                                &format!(
                                    "scheme={} host={} port={}",
                                    url.scheme(),
                                    url.host_str().unwrap_or_default(),
                                    url.port_or_known_default()
                                        .map_or_else(|| "-".to_string(), |port| port.to_string())
                                ),
                            );
                            false
                        }
                    }
                })
                .build()
        })
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("startup-probe-single-instance")
                .setup(|_app, _api| {
                    startup::mark("tauri:plugin_single_instance_ready");
                    Ok(())
                })
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("startup-probe-notification")
                .setup(|_app, _api| {
                    startup::mark("tauri:plugin_notification_ready");
                    Ok(())
                })
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("startup-probe-dialog")
                .setup(|_app, _api| {
                    startup::mark("tauri:plugin_dialog_ready");
                    Ok(())
                })
                .build(),
        )
        .on_page_load(|webview, payload| {
            startup::mark_with_detail(
                "rust",
                "webview:page_load",
                &format!("label={} event={:?}", webview.label(), payload.event()),
            );
        })
        .setup(|app| {
            startup::mark("setup:start");
            #[cfg(target_os = "macos")]
            features::updater::cleanup_stale_backup();
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            startup::mark("setup:plugins_ready");

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
            startup::mark("workflow_migrate:start");
            if let Err(e) = crate::features::workflow::workflow_migrate::migrate_if_needed() {
                eprintln!("[pinvou3-app] workflow migrate failed (will retry next boot): {e}");
                startup::mark_with_detail("rust", "workflow_migrate:error", &e.to_string());
            }
            startup::mark("workflow_migrate:done");

            // 多对话历史 store：用 ~/.pinvou3/sessions/ 隔离 deepseek-tui 全局目录。
            // 必须先 boot 这个，engine forwarder 需要它跟踪 active session 的 mode_state
            // 以便 TurnComplete 时判定是否 emit chat:plan_ready。
            startup::mark("session_store:start");
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
            startup::mark("session_store:done");
            if let Some(store) = session_store.clone() {
                app.handle().manage(store);
            }
            let remote_control_manager = RemoteControlManager::new(app.handle().clone());
            let remote_event_transport = remote_control_manager.clone();
            app.handle().manage(platform::app_events::AppEventBus::new(
                move |event, payload| remote_event_transport.forward_local_event(event, payload),
            ));
            app.handle().manage(remote_control_manager.clone());
            // 多 session 并发:存 EnginePool(lazy spawn,首条消息才为该 session 起 engine)。
            // boot bridge 在 pool::new 里做一次(写盘 / 设 env 只能一次)。
            let handle = app.handle().clone();
            let store_for_engine = session_store.unwrap_or_else(|| {
                // store boot 失败时退化用一份临时 store（让 engine 至少能起来）；
                // 实际使用 session 相关命令会失败,但聊天能跑
                SessionStore::boot().expect("session store boot fallback")
            });
            match crate::features::codex_acp::AcpPool::new(handle.clone(), store_for_engine.clone())
            {
                Ok(pool) => {
                    handle.manage(pool);
                    eprintln!("[pinvou3-app] Codex ACP pool ready (lazy spawn per session)");
                }
                Err(error) => {
                    panic!("failed to init Codex ACP pool: {error:#}");
                }
            }
            startup::mark("engine_pool:start");
            let tool_factory: crate::features::assistant::engine_pool::EngineToolFactory =
                std::sync::Arc::new(|app, session_id| {
                    vec![
                        std::sync::Arc::new(knowledge::KbSearchTool::new(
                            app.clone(),
                            session_id.to_string(),
                        )),
                        std::sync::Arc::new(knowledge::KbOpenSourceTool::new(
                            app.clone(),
                            session_id.to_string(),
                        )),
                    ]
                });
            let tool_policy: crate::features::assistant::engine_pool::ToolPolicy =
                std::sync::Arc::new(|app| {
                    let mut tools = crate::features::marketplace::disabled_tool_names();
                    let kb_usable = app
                        .try_state::<knowledge::KnowledgeService>()
                        .map(|service| service.has_indexed_content() && service.semantic_ready())
                        .unwrap_or(false);
                    if !kb_usable {
                        tools.push("kb_search".to_string());
                        tools.push("kb_open_source".to_string());
                    }
                    tools
                });
            match EnginePool::new_with_dependencies(
                handle.clone(),
                store_for_engine.clone(),
                tool_factory,
                tool_policy,
            ) {
                Ok(pool) => {
                    let scheduled_state = tauri::async_runtime::block_on(
                        scheduled_tasks::ScheduledTaskState::boot_runtime(
                            &pool.bridge,
                            pool.clone(),
                            store_for_engine.clone(),
                        ),
                    );
                    match scheduled_state {
                        Ok(state) => {
                            handle.manage(state);
                            eprintln!("[pinvou3-app] scheduled tasks runtime ready");
                        }
                        Err(e) => {
                            eprintln!("[pinvou3-app] scheduled tasks runtime init failed: {e:?}");
                        }
                    }
                    handle.manage(pool);
                    eprintln!("[pinvou3-app] engine pool ready (lazy spawn per session)");
                    match remote_control_manager.resume() {
                        Ok(true) => eprintln!("[pinvou3-app] persistent Web access resumed"),
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("[pinvou3-app] persistent Web access resume failed: {error}")
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[pinvou3-app] failed to init engine pool: {e:?}");
                }
            }
            startup::mark("engine_pool:done");

            // 技能停用联动:启动时按当前被禁用连接器的 companion_skills 推给底座进程级
            // 过滤器,让(如公文 MCP 关掉时的)关联技能从首轮 prompt 起就不出现在 ## Skills。
            startup::mark("disabled_skills:start");
            crate::features::marketplace::skill_marketplace::refresh_disabled_skills();
            startup::mark("disabled_skills:done");

            // Monitor 按需采样：state 只持有 session_uptime，sample 由前端调
            // get_monitor_snapshot 时触发（监控页面 1s interval，离开页面停）。
            let monitor_state = MonitorState::new();
            app.handle().manage(monitor_state);
            app.handle()
                .manage(notifications::NotificationState::default());
            app.handle()
                .manage(pet_window::PetNavigationState::default());
            app.handle().manage(pet_window::PetReplyState::default());
            app.handle().manage(selected_pet::SelectedPetStore::load());

            // CLI 连接器连接编排状态(按连接器 id 存长驻子进程 PID + 取消标志),
            // 飞书 / 企微共用,供 *_connect_begin / *_cancel 用。
            app.handle().manage(connector_cli::ConnectorConn::default());

            // 工作流 Phase 可视化:skill 绑定挂在 SessionStore.mode_state 上,
            // per-session 隔离(start_skill_session 命令负责新建 session + bind)。
            // 不再需要全局 ActiveSkillStore。

            // File watcher: 监听 ~/.pinvou3/sessions/ 树,新文件 emit artifact:disk
            file_watcher::spawn(app.handle().clone(), bridge::paths::sessions_root());
            startup::mark("file_watcher:spawned");

            // 本地知识底座 L0:全系统元数据索引(秒搜+去重)。这里只 manage,**不自动扫**——
            // 扫描改懒触发:由前端进入文件管理页时增量扫(不进页=零扫描),不常驻 watcher/周期
            // 重扫。文件管理是低频功能,不该长期占资源。
            // embedding 模型**不再随 deb 打包**(deb 瘦 ~559MB):改按需下载到
            // ~/.pinvou3/knowledge/models/bge-m3。setup 只打开数据库并以 embedder=None 注册服务；
            // React 首帧后再调用 kb_model_load_after_first_frame，通过 spawn_blocking 后台加载。
            // 模型没装/加载失败时维持完全门控，不阻断启动；下载完成仍可热加载。
            // 语音识别引擎随平台安装包打包,容错同 bge-m3 的资源布局,
            // 注入给 voice_asr 作为 ~/.pinvou3/asr/ 之外的回退查找目录。
            if let Some(asr_res) = app.path().resource_dir().ok().and_then(|res| {
                [
                    res.join("runtime").join("asr"),
                    res.join("resources").join("runtime").join("asr"),
                ]
                .into_iter()
                .find(|d| d.join(features::voice::engine_binary_name()).exists())
            }) {
                features::voice::set_bundled_engine_dir(asr_res);
            }

            startup::mark("knowledge_service:start");
            match knowledge::KnowledgeService::new(&knowledge::default_db_path()) {
                Ok(svc) => {
                    app.handle().manage(svc);
                    eprintln!("[pinvou3-app] knowledge service ready");
                }
                Err(e) => eprintln!("[pinvou3-app] knowledge service init failed: {e:?}"),
            }
            startup::mark("knowledge_service:done");

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
                std::thread::spawn(move || {
                    startup::mark("web_template_seed:start");
                    seed_web_template(web_tpl_src);
                    startup::mark("web_template_seed:done");
                });
            }

            // 桌宠:settings.json 里 pet.enabled 为真时随主窗口一起拉起。
            pet_window::spawn_if_enabled(app.handle());

            startup::mark("setup:done");
            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口销毁 → 一并关掉桌宠,否则只剩宠物窗口时 app 不退出。
            if window.label() == "main" {
                if let tauri::WindowEvent::Destroyed = event {
                    pet_window::close_with_main(window.app_handle());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat::chat,
            commands::startup::report_frontend_startup,
            commands::connectors::refresh_connector_auth_gates,
            commands::connectors::feishu_ensure_cli,
            commands::connectors::feishu_status,
            commands::connectors::feishu_connect_begin,
            commands::connectors::feishu_cancel,
            commands::connectors::feishu_logout,
            commands::connectors::feishu_apply_skills,
            commands::connectors::set_feishu_enabled,
            commands::connectors::feishu_skills_state,
            commands::connectors::wecom_ensure_cli,
            commands::connectors::wecom_status,
            commands::connectors::wecom_connect_begin,
            commands::connectors::wecom_cancel,
            commands::connectors::wecom_logout,
            commands::connectors::wecom_apply_skills,
            commands::connectors::set_wecom_enabled,
            commands::connectors::wecom_skills_state,
            commands::connectors::dingtalk_ensure_cli,
            commands::connectors::dingtalk_status,
            commands::connectors::dingtalk_connect_begin,
            commands::connectors::dingtalk_cancel,
            commands::connectors::dingtalk_logout,
            commands::connectors::dingtalk_apply_skills,
            commands::connectors::set_dingtalk_enabled,
            commands::connectors::dingtalk_skills_state,
            commands::connectors::tmeet_ensure_cli,
            commands::connectors::tmeet_status,
            commands::connectors::tmeet_connect_begin,
            commands::connectors::tmeet_cancel,
            commands::connectors::tmeet_logout,
            commands::connectors::tmeet_apply_skills,
            commands::connectors::set_tmeet_enabled,
            commands::connectors::tmeet_skills_state,
            commands::connectors::ima_status,
            commands::connectors::ima_connect,
            commands::connectors::ima_logout,
            commands::settings::get_settings,
            commands::runtime::get_platform_capabilities,
            commands::settings::submit_feedback,
            commands::settings::get_effective_model_config,
            commands::settings::update_settings,
            commands::settings::save_settings_and_restart,
            commands::sessions::clear_session,
            commands::monitor::get_monitor_snapshot,
            commands::monitor::get_backend_status,
            commands::monitor::discover_local_vllm,
            commands::local_llm::detect_local_vllm_setup,
            commands::local_llm::bootstrap_local_vllm,
            commands::local_llm::decline_local_vllm_setup,
            commands::settings::list_models,
            commands::settings::reveal_model_api_key,
            commands::settings::save_model,
            commands::settings::delete_model,
            commands::settings::set_active_model,
            commands::settings::set_session_model,
            commands::settings::get_session_model_id,
            commands::codex::get_codex_acp_status,
            commands::codex::prepare_codex_acp,
            commands::codex::login_codex_acp,
            commands::codex::open_codex_login_url,
            commands::codex::get_codex_acp_session_info,
            commands::codex::set_codex_acp_model,
            commands::codex::set_codex_acp_mode,
            commands::codex::set_codex_acp_config_option,
            commands::codex::codex_acp_prompt,
            commands::codex::cancel_codex_acp,
            commands::codex::get_codex_acp_timeline,
            commands::codex::get_codex_acp_pending_permissions,
            commands::codex::respond_codex_acp_permission,
            commands::codex::get_codex_acp_pending_elicitations,
            commands::codex::respond_codex_acp_elicitation,
            commands::codex::list_codex_acp_sessions,
            commands::codex::create_codex_acp_session,
            commands::codex::list_codex_workspace,
            commands::codex::search_codex_workspace,
            commands::codex::preview_codex_workspace_file,
            commands::codex::get_codex_workspace_changes,
            commands::codex::get_codex_workspace_diff,
            commands::codex::open_codex_workspace_file,
            commands::codex::reveal_codex_workspace_file,
            commands::codex::reconcile_codex_acp_artifacts,
            commands::settings::test_model_connection,
            commands::settings::test_search_provider,
            commands::voice::transcribe_voice_audio,
            commands::voice::reset_microphone_permission,
            commands::voice::voice_asr_status,
            commands::voice::install_voice_asr,
            commands::voice::cancel_voice_asr,
            commands::sessions::list_sessions,
            commands::sessions::create_session,
            commands::sessions::load_session,
            commands::sessions::delete_session,
            commands::sessions::rename_session,
            commands::sessions::set_session_pinned,
            commands::sessions::list_archived_sessions,
            commands::sessions::set_session_archived,
            commands::timeline::get_session_timeline,
            commands::timeline::get_session_stats,
            commands::scheduled::list_scheduled_tasks,
            commands::scheduled::read_scheduled_task,
            commands::scheduled::list_scheduled_task_runs,
            commands::scheduled::list_scheduled_runs,
            commands::scheduled::create_scheduled_task,
            commands::scheduled::update_scheduled_task,
            commands::scheduled::pause_scheduled_task,
            commands::scheduled::resume_scheduled_task,
            commands::scheduled::set_scheduled_task_pinned,
            commands::scheduled::delete_scheduled_task,
            commands::scheduled::run_scheduled_task_now,
            commands::scheduled::mark_scheduled_run_viewed,
            commands::scheduled::scheduled_task_chat_prompt,
            commands::sessions::get_active_session,
            commands::sessions::save_session_messages,
            commands::sessions::save_session_artifacts,
            commands::sessions::list_workspace_files,
            commands::runtime::cancel_generation,
            commands::runtime::list_shell_tasks,
            commands::runtime::cancel_shell_task,
            commands::remote_control::web_access_enable,
            commands::remote_control::web_access_disable,
            commands::remote_control::web_access_status,
            commands::remote_control::web_access_rotate,
            commands::remote_control::web_access_relay_settings,
            commands::remote_control::web_access_set_relay,
            commands::remote_control::web_access_reset_relay,
            commands::remote_control::web_access_bridge_ready,
            commands::remote_control::web_access_rpc_begin,
            commands::remote_control::web_access_rpc_respond,
            commands::remote_control::web_access_publish_event,
            commands::remote_control::web_access_list_host_files,
            commands::remote_control::web_access_create_session,
            commands::remote_control::web_access_load_session_chunk,
            commands::remote_control::web_access_ingest_file,
            commands::remote_control::web_access_chat,
            commands::remote_control::web_access_save_session_messages_chunk,
            commands::remote_control::web_access_transcribe_voice_audio,
            commands::remote_control::web_access_start_skill_session,
            commands::remote_control::web_access_read_artifact_chunk,
            commands::remote_control::web_access_update_settings,
            commands::remote_control::web_access_artifact_info,
            commands::remote_control::web_access_read_artifact_text,
            commands::remote_control::web_access_write_artifact_text,
            commands::remote_control::web_access_read_artifact_image_b64,
            commands::remote_control::web_access_read_artifact_thumbnail,
            commands::remote_control::web_access_render_artifact_visual,
            commands::remote_control::web_access_list_deliverables,
            commands::remote_control::web_access_get_role_prompt,
            commands::remote_control::web_access_get_role_outputs,
            commands::remote_control::web_access_get_role_logs,
            commands::remote_control::web_access_get_gate_report,
            commands::connectors::set_disabled_connectors,
            commands::connectors::get_disabled_connectors,
            commands::memory::get_memory_profile,
            commands::memory::update_memory_profile,
            commands::memory::clear_memory_profile,
            commands::memory::get_memory_overview,
            commands::memory::list_pending_memory,
            commands::memory::suggest_memory,
            commands::memory::confirm_pending_memory,
            commands::memory::ignore_pending_memory,
            commands::memory::never_pending_memory,
            commands::memory::list_recent_work_memory,
            commands::memory::upsert_recent_work_memory,
            commands::memory::archive_recent_work_memory,
            commands::memory::delete_memory_preference,
            commands::memory::update_memory_preference,
            commands::memory::update_work_context_memory,
            commands::memory::delete_work_context_memory,
            commands::memory::update_timed_memory,
            commands::memory::delete_timed_memory,
            commands::memory::edit_last_turn,
            commands::artifacts::read_artifact_text,
            commands::artifacts::write_artifact_text,
            commands::artifacts::list_deliverables,
            commands::artifacts::list_deliverable_index,
            commands::artifacts::artifact_info,
            commands::artifacts::render_artifact_visual,
            commands::artifacts::read_artifact_image_b64,
            commands::artifacts::read_artifact_thumbnail,
            commands::artifacts::open_in_system,
            commands::artifacts::open_containing_folder,
            commands::artifacts::reveal_session_folder,
            commands::artifacts::open_scheduled_task_folder,
            commands::artifacts::open_artifact_window,
            commands::pet::open_detached_window,
            commands::pet::begin_detach_drag,
            commands::pet::set_pet_enabled,
            commands::pet::get_pet_scale,
            commands::pet::set_pet_scale,
            commands::pet::set_pet_activity_visible,
            commands::pet::save_pet_position,
            commands::pet::save_pet_vertical_alignment,
            commands::pet::open_main_from_pet,
            commands::pet::take_pet_navigation,
            commands::pet::queue_pet_reply,
            commands::pet::take_pet_reply,
            commands::pet::get_selected_pet,
            commands::pet::set_selected_pet,
            commands::artifacts::open_external_url,
            commands::files::ingest_file,
            commands::files::detect_system_tools,
            commands::files::save_paste_image,
            commands::interaction::compact_now,
            commands::interaction::get_mode_state,
            commands::interaction::set_plan_mode_next,
            commands::interaction::exit_plan_to_yolo,
            commands::interaction::accept_plan,
            commands::interaction::discard_plan,
            commands::interaction::read_skill_body,
            commands::workflows::list_skills_v2,
            commands::workflows::read_skill_demo,
            commands::workflows::start_skill_session,
            commands::workflows::unbind_session_skill,
            commands::workflows::list_workflows,
            commands::workflows::start_workflow,
            commands::workflows::kick_workflow,
            commands::workflows::retry_workflow_role,
            commands::workflows::get_role_prompt,
            commands::workflows::get_role_outputs,
            commands::workflows::get_role_logs,
            commands::workflows::get_gate_report,
            commands::workflows::save_project_config,
            commands::workflows::save_agent_overrides,
            commands::workflows::cancel_workflow_role,
            commands::workflows::stop_workflow,
            commands::workflows::approve_workflow_gate,
            commands::workflows::reject_workflow_gate,
            commands::workflows::get_workflow_state,
            commands::workflows::find_resumable_run,
            commands::workflows::get_session_active_skill,
            commands::workflows::list_session_skill_bindings,
            commands::interaction::submit_user_input,
            commands::interaction::add_run_materials,
            commands::interaction::cancel_user_input,
            commands::interaction::restart_engine,
            commands::interaction::summon_pinvou,
            commands::personas::save_session_pinvou_reviews,
            commands::personas::get_session_pinvou_reviews,
            commands::interaction::get_super_permission_status,
            commands::interaction::set_super_permission,
            commands::personas::list_personas,
            commands::personas::read_persona_body,
            commands::personas::equip_persona,
            commands::personas::unequip_persona,
            commands::personas::get_active_persona,
            commands::personas::create_persona,
            commands::personas::update_persona,
            commands::personas::delete_persona,
            commands::personas::save_session_persona_events,
            commands::personas::get_session_persona_events,
            commands::updater::get_app_version,
            commands::updater::check_for_update,
            commands::updater::download_update,
            commands::updater::install_update,
            commands::updater::restart_app,
            commands::updater::cancel_download,
            commands::updater::report_pending_update_result,
            commands::dependencies::check_dependencies,
            commands::dependencies::install_dependencies,
            commands::marketplace::list_marketplace_tools,
            commands::marketplace::install_marketplace_tool,
            commands::marketplace::get_marketplace_tool_auth_status,
            commands::marketplace::start_marketplace_tool_oauth_login,
            commands::marketplace::cancel_marketplace_tool_oauth_login,
            commands::marketplace::uninstall_marketplace_tool,
            commands::artifacts::detect_obsidian,
            commands::knowledge::kb_start_scan,
            commands::knowledge::kb_scan_status,
            commands::knowledge::kb_cancel_scan,
            commands::knowledge::kb_search,
            commands::knowledge::kb_stats,
            commands::knowledge::kb_type_counts,
            commands::knowledge::kb_collection_list,
            commands::knowledge::kb_collection_create,
            commands::knowledge::kb_collection_update,
            commands::knowledge::kb_collection_delete,
            commands::knowledge::kb_collection_add_sources,
            commands::knowledge::kb_index_status,
            commands::knowledge::kb_index_cancel,
            commands::knowledge::kb_documents,
            commands::knowledge::kb_remove_document,
            commands::knowledge::kb_embed_info,
            commands::knowledge::kb_model_status,
            commands::knowledge::kb_model_load_after_first_frame,
            commands::knowledge::kb_model_download,
            commands::knowledge::kb_model_cancel,
            commands::knowledge::session_mount_collection,
            commands::knowledge::session_unmount_collection,
            commands::knowledge::session_mounted_collection,
            commands::marketplace::list_marketplace_skills,
            commands::marketplace::install_marketplace_skill,
            commands::marketplace::import_skill_package,
            commands::marketplace::uninstall_marketplace_skill,
            commands::files::verify_upload,
        ]);

    startup::mark("tauri:builder_configured");
    startup::mark("tauri:context:start");
    let context = tauri::generate_context!();
    startup::mark("tauri:context:done");
    // Keep the historical marker so old and new startup runs remain comparable.
    startup::mark("tauri:run_enter");
    startup::mark("tauri:build:start");
    let app = builder
        .build(context)
        .expect("error while building tauri application");
    startup::mark("tauri:build:done");
    startup::mark("tauri:event_loop:run_enter");
    let mut resumed_reported = false;
    app.run(move |_app, event| match event {
        tauri::RunEvent::Ready => startup::mark("tauri:event_loop:ready"),
        tauri::RunEvent::Resumed if !resumed_reported => {
            resumed_reported = true;
            startup::mark("tauri:event_loop:first_resumed");
        }
        _ => {}
    });
    startup::mark("process:exit");
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
            "agent_open",     // subagent spawn(单一 spawn 入口)
            "agent_eval",     // subagent 收结果
            "agent_close",    // subagent 释放 session
            "kb_search",      // Agentic RAG: app 注入的本地知识检索工具,必须对模型可见
            "kb_open_source", // 只按受控 source_ref 展开知识文档 chunk,禁止退回二进制 read_file
        ] {
            assert!(!is_pinvou3_hidden(core), "核心工具 {core} 不应该被隐藏");
        }
    }
}

#[cfg(test)]
mod web_template_seed {
    #[cfg(unix)]
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
        assert!(crate::platform::paths::web_template_dir().ends_with("web-template"));
    }
}
