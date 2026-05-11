//! Engine 工厂 — 从环境变量构建 PlatformEngine 实例。
//!
//! 环境变量:
//!   DEEPSEEK_API_KEY   — API key（必填）
//!   DEEPSEEK_BASE_URL  — API 端点（可选，默认 DeepSeek 官方）
//!   DEEPSEEK_MODEL     — 模型名（可选，默认 deepseek-chat）

use std::path::PathBuf;

use anyhow::Result;
use deepseek_tui::client::DeepSeekClient;
use deepseek_tui::config::Config;

use crate::app::AppRegistry;
use crate::deepseek_harness::DeepSeekHarness;
use crate::engine::PlatformEngine;
use crate::harness::{ModelInfo, ToolDef};

pub type PinvouEngine = PlatformEngine<DeepSeekHarness<DeepSeekClient>>;

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

    Ok(PlatformEngine::new(harness, registry, workspace))
}
