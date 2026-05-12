//! pinvou3-app 与 DeepSeek-TUI Engine 的桥接层。
//!
//! 职责（极简版，Week 1）：
//!  1. 从环境变量构造 [`EngineConfig`] 并 `spawn_engine`，存到 Tauri State。
//!  2. 后台 task 持续读 `EngineHandle::rx_event`，把 Engine 事件转译成 Tauri 事件
//!     （`chat:delta` / `chat:tool_start` / `chat:tool_end` / `chat:done`）。
//!  3. 暴露 `send_user_message()` 给 `commands::chat` 调用。
//!
//! Week 1 只接 Engine 自带 ToolRegistry（不 BYO）、不做多 session、不做审批。
//! Engine 自管 session 状态，所以多轮对话在同一个 EngineHandle 内自然累积。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use deepseek_tui::config::{ApiProvider, Config as DtConfig, ProvidersConfig};
use deepseek_tui::core::engine::{spawn_engine, EngineConfig, EngineHandle};
use deepseek_tui::core::events::Event;
use deepseek_tui::core::ops::Op;
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;
use serde_json::json;
use tauri::{AppHandle, Emitter};

/// 由 Tauri State 持有，前端通过 `invoke('chat', ...)` 间接调它。
#[derive(Clone)]
pub struct AppEngine {
    pub handle: EngineHandle,
    pub model: String,
    pub workspace: PathBuf,
}

impl AppEngine {
    /// 从环境变量构造 Engine 并 spawn 后台任务（含 event forwarding）。
    /// 必须在 Tauri `setup()` 内异步上下文里调。
    pub async fn spawn(app: AppHandle) -> Result<Self> {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (config, model) = build_dt_config_from_env();

        // 给 Engine 用的最小 EngineConfig：trust_mode=true 允许 workspace 内读写，
        // allow_shell=true 让 LLM 能跑 exec_shell（pinvou3 MVP YOLO 模式）。
        let engine_config = EngineConfig {
            model: model.clone(),
            workspace: workspace.clone(),
            allow_shell: true,
            trust_mode: true,
            ..EngineConfig::default()
        };

        eprintln!(
            "[pinvou3-app] spawn_engine model={} workspace={}",
            model,
            workspace.display()
        );
        let handle = spawn_engine(engine_config, &config);

        // 启动事件转发后台 task：从 rx_event 拉 Event 转 Tauri emit
        spawn_event_forwarder(app, handle.clone());

        Ok(Self {
            handle,
            model,
            workspace,
        })
    }

    /// 发用户消息给 Engine。Engine 内部自管 session 状态，所以多轮自然累积。
    pub async fn send_user_message(&self, content: String) -> Result<()> {
        self.handle
            .send(Op::SendMessage {
                content,
                mode: AppMode::Agent,
                model: self.model.clone(),
                goal_objective: None,
                reasoning_effort: Some("off".to_string()),
                reasoning_effort_auto: false,
                auto_model: false,
                allow_shell: true,
                trust_mode: true,
                auto_approve: true,
                approval_mode: ApprovalMode::Auto,
            })
            .await?;
        Ok(())
    }
}

/// 后台 task：持续读 rx_event 转 Tauri emit。
fn spawn_event_forwarder(app: AppHandle, handle: EngineHandle) {
    tauri::async_runtime::spawn(async move {
        let mut rx = handle.rx_event.write().await;
        while let Some(event) = rx.recv().await {
            match event {
                Event::MessageDelta { content, .. } => {
                    let _ = app.emit("chat:delta", json!({ "text": content }));
                }
                Event::ThinkingDelta { .. } => {
                    // Week 1 丢弃 thinking 段（Qwen3 已用 reasoning_effort=off 关掉）
                }
                Event::ToolCallStarted { id, name, input } => {
                    let _ = app.emit(
                        "chat:tool_start",
                        json!({ "id": id, "name": name, "args": input }),
                    );
                }
                Event::ToolCallComplete { id, name, result } => {
                    let (output, success) = match result {
                        Ok(r) => (r.content, true),
                        Err(e) => (format!("{e:?}"), false),
                    };
                    let _ = app.emit(
                        "chat:tool_end",
                        json!({ "id": id, "name": name, "output": output, "success": success }),
                    );
                }
                Event::TurnComplete { status, error, .. } => {
                    let _ = app.emit(
                        "chat:done",
                        json!({ "status": format!("{status:?}"), "error": error }),
                    );
                }
                Event::Error { envelope, .. } => {
                    let _ = app.emit(
                        "chat:done",
                        json!({ "status": "error", "error": envelope.message }),
                    );
                }
                _ => {} // 其他事件忽略
            }
        }
        eprintln!("[pinvou3-app] event forwarder stopped (engine shut down?)");
    });
}

/// 从环境变量构造 DeepSeek-TUI Config + 解析 model 名。
/// 复用 pinvou-platform/engine_factory.rs 的 BASE_URL 按 provider 分流逻辑。
fn build_dt_config_from_env() -> (DtConfig, String) {
    let mut config = DtConfig::default();

    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        config.api_key = Some(key);
    }
    if let Ok(p) = std::env::var("DEEPSEEK_PROVIDER") {
        config.provider = Some(p);
    }
    if let Ok(url) = std::env::var("DEEPSEEK_BASE_URL") {
        let provider = config
            .provider
            .as_deref()
            .and_then(ApiProvider::parse)
            .unwrap_or(ApiProvider::Deepseek);
        let providers = config.providers.get_or_insert_with(ProvidersConfig::default);
        match provider {
            ApiProvider::Openai => providers.openai.base_url = Some(url),
            ApiProvider::NvidiaNim => providers.nvidia_nim.base_url = Some(url.clone()),
            ApiProvider::Openrouter => providers.openrouter.base_url = Some(url),
            ApiProvider::Novita => providers.novita.base_url = Some(url),
            ApiProvider::Fireworks => providers.fireworks.base_url = Some(url),
            ApiProvider::Sglang => providers.sglang.base_url = Some(url),
            ApiProvider::Vllm => providers.vllm.base_url = Some(url),
            ApiProvider::Ollama => providers.ollama.base_url = Some(url),
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
                config.base_url = Some(url);
            }
        }
    }

    let model = std::env::var("DEEPSEEK_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| config.default_model());

    config.default_text_model = Some(model.clone());

    // 把 reasoning_effort=off 也注入 config（虽然 SendMessage 也会直接传一份）
    config.reasoning_effort = Some("off".to_string());

    let base_url = config.deepseek_base_url();
    eprintln!("[pinvou3-app] API: {base_url}");
    eprintln!("[pinvou3-app] Model: {model}");

    (config, model)
}

/// 让 main.rs 编译时知道这个模块（供 docs/CI 用）。
pub fn _force_link() -> Arc<()> {
    Arc::new(())
}
