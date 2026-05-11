//! Engine 工厂 — 从环境变量构建 PlatformEngine 实例。
//!
//! 环境变量:
//!   DEEPSEEK_API_KEY   — API key（必填）
//!   DEEPSEEK_BASE_URL  — API 端点（可选，默认 DeepSeek 官方）
//!   DEEPSEEK_MODEL     — 模型名（可选，默认 deepseek-chat）

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use deepseek_tui::client::DeepSeekClient;
use deepseek_tui::config::Config;

use crate::agent_registry::AgentRegistry;
use crate::app::AppRegistry;
use crate::deepseek_harness::DeepSeekHarness;
use crate::engine::PlatformEngine;
use crate::harness::{ModelInfo, ToolDef};

pub type PinvouEngine = PlatformEngine<DeepSeekHarness<DeepSeekClient>>;

/// 加载 prompts/ 目录下的 agent。若目录不存在或为空，返回空 registry（不报错）。
pub fn load_agents(prompts_dir: impl AsRef<Path>) -> Arc<AgentRegistry> {
    let dir = prompts_dir.as_ref();
    let reg = if dir.exists() {
        match AgentRegistry::from_directory(dir) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "[pinvou3] 加载 prompts 目录失败 ({}): {e}",
                    dir.display()
                );
                AgentRegistry::default()
            }
        }
    } else {
        eprintln!("[pinvou3] prompts/ 目录不存在: {}", dir.display());
        AgentRegistry::default()
    };
    eprintln!("[pinvou3] 已加载 {} 个 agent", reg.len());
    Arc::new(reg)
}

/// 从环境变量创建引擎。
pub fn create_engine(registry: AppRegistry, workspace: PathBuf) -> Result<PinvouEngine> {
    let mut config = Config::default();

    // 从环境变量覆盖配置
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        config.api_key = Some(key);
    }
    if let Ok(url) = std::env::var("DEEPSEEK_BASE_URL") {
        config.base_url = Some(url);
    }

    let model_name = std::env::var("DEEPSEEK_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| config.default_model());

    // 关键：设入 Config，DeepSeekClient 才能用对模型名
    config.default_text_model = Some(model_name.clone());

    let base_url = config.deepseek_base_url();
    eprintln!("[pinvou3] API: {base_url}");
    eprintln!("[pinvou3] Model: {model_name}");

    let client = DeepSeekClient::new(&config)?;

    let tool_names: Vec<ToolDef> = vec![
        ToolDef {
            name: "request_user_input".into(),
            description: "Ask the user 1-3 short questions and return their selections. Use this when you need user decisions — never ask open-ended text questions.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 3,
                        "items": {
                            "type": "object",
                            "properties": {
                                "header": { "type": "string", "description": "Short label (max 12 chars)" },
                                "id": { "type": "string", "description": "Unique identifier for this question" },
                                "question": { "type": "string", "description": "The question to ask the user" },
                                "options": {
                                    "type": "array",
                                    "minItems": 2,
                                    "maxItems": 3,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string", "description": "Short option label" },
                                            "description": { "type": "string", "description": "What this option means" }
                                        },
                                        "required": ["label", "description"]
                                    }
                                }
                            },
                            "required": ["header", "id", "question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        },
    ];

    let models = vec![ModelInfo {
        id: model_name,
        provider: "deepseek".into(),
        capability: "large".into(),
    }];

    let harness = DeepSeekHarness::new(client, tool_names, models, workspace.clone());

    let mut engine = PlatformEngine::new(harness, registry, workspace.clone());

    // 注入 AgentRegistry（默认从 workspace/prompts 加载）
    let prompts_dir = workspace.join("prompts");
    engine.set_agent_registry(load_agents(&prompts_dir));

    Ok(engine)
}
