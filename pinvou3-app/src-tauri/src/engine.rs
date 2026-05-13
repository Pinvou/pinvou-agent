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

        // 自动定位项目根：cwd 通常是 pinvou3-app/src-tauri/（cargo run 时），
        // 项目根的标志是同时含 CLAUDE.md + DeepSeek-TUI/。
        let project_root = workspace.ancestors().find(|p| {
            p.join("CLAUDE.md").is_file() && p.join("DeepSeek-TUI").is_dir()
        });

        // 加载项目级 instructions.md（如果存在）。
        // 这是 CLAUDE.md 约束 1 "改 LLM 行为引导 → .deepseek/instructions.md" 的兑现：
        // 不写 Rust 改 LLM prompt，靠这个 markdown 文件强化 Qwen3.6 的 agent 行为。
        let instructions: Vec<PathBuf> = project_root
            .map(|p| p.join(".deepseek").join("instructions.md"))
            .filter(|p| p.is_file())
            .map(|p| vec![p])
            .unwrap_or_default();

        // 给 Engine 用的最小 EngineConfig：trust_mode=true 允许 workspace 内读写，
        // allow_shell=true 让 LLM 能跑 exec_shell（pinvou3 MVP YOLO 模式）。
        let engine_config = EngineConfig {
            model: model.clone(),
            workspace: workspace.clone(),
            allow_shell: true,
            trust_mode: true,
            instructions: instructions.clone(),
            ..EngineConfig::default()
        };

        eprintln!(
            "[pinvou3-app] spawn_engine model={} workspace={} instructions={}",
            model,
            workspace.display(),
            if instructions.is_empty() {
                "none".to_string()
            } else {
                instructions
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            }
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
    ///
    /// 用 `AppMode::Yolo`（不是 Agent）：pinvou3 MVP 是 YOLO 行为（auto_approve=true，
    /// trust_mode=true），且 DeepSeek-TUI 在 Yolo 模式下禁用 deferred tool loading
    /// （tool_catalog.rs:32 `if mode == AppMode::Yolo { return false }`）。
    /// Agent 模式 + Qwen3.6 会触发 write_file 等延迟工具的"加载后重试"流程，加上
    /// 上游 schema validator 对字段顺序敏感，会导致 LLM 反复 retry 卡死。
    pub async fn send_user_message(&self, content: String) -> Result<()> {
        self.handle
            .send(Op::SendMessage {
                content,
                mode: AppMode::Yolo,
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
    let approve_handle = handle.clone();
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
                Event::ApprovalRequired {
                    id, tool_name, ..
                } => {
                    // pinvou3 是 YOLO 助手 —— Engine 的 SendMessage.auto_approve=true
                    // 实际上不旁路 await_tool_approval（turn_loop.rs:1117 只看 ToolSpec
                    // 自己的 approval_requirement，不看 session.auto_approve）。需要
                    // 我们 frontend 主动发 ApprovalDecision::Approved 才能解锁工具执行。
                    eprintln!(
                        "[pinvou3-app] auto-approving tool {} id={}",
                        tool_name, id
                    );
                    let h = approve_handle.clone();
                    let id_clone = id.clone();
                    tokio::spawn(async move {
                        if let Err(e) = h.approve_tool_call(id_clone).await {
                            eprintln!("[pinvou3-app] approve_tool_call failed: {e:?}");
                        }
                    });
                    // 同时通知前端"有工具开始"——engine 的 ToolCallStarted 会随后到，
                    // 这条主要是用户感知（避免审批阶段静默）
                    let _ = app.emit(
                        "chat:tool_start",
                        json!({ "id": id, "name": tool_name, "args": null }),
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
