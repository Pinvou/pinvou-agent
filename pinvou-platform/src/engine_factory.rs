//! Engine 工厂 — 从环境变量构建 PlatformEngine 实例。
//!
//! 环境变量:
//!   DEEPSEEK_API_KEY   — API key（必填）
//!   DEEPSEEK_BASE_URL  — API 端点（可选，默认 DeepSeek 官方）
//!   DEEPSEEK_MODEL     — 模型名（可选，默认 deepseek-chat）

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use deepseek_tui::client::DeepSeekClient;
use deepseek_tui::config::Config;
use deepseek_tui::tools::registry::{ToolRegistry, ToolRegistryBuilder};
use deepseek_tui::tools::spec::ToolContext;

use crate::agent_registry::AgentRegistry;
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
pub fn create_engine(workspace: PathBuf) -> Result<PinvouEngine> {
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

    // 构造 DeepSeek-TUI 工具注册表（批量挂多个工具）
    let (tool_registry, tool_context, auto_tool_names) = build_default_tool_registry(&workspace);

    let harness = DeepSeekHarness::new(client, tool_names, models, workspace.clone())
        .with_tools(tool_registry, tool_context, auto_tool_names);

    // 注意：AgentRegistry 由 main.rs 用 CLI 的 `--prompts-dir` 显式注入，
    // create_engine 不再这里加载，避免重复读盘 + 重复日志。
    Ok(PlatformEngine::new(harness, workspace.clone()))
}

/// 构造默认工具注册表 + 执行上下文。
///
/// 默认 YOLO 模式（`auto_approve=true`, `trust_mode=true`），不弹审批对话框。
/// pinvou3 是本地单用户工具，workspace 是天然边界。如果未来需要更严的审批，
/// 在这里切换 `with_auto_approve(...)` 的参数。
///
/// **自动执行**的工具（harness 直接 `ToolSpec::execute()`，结果写回对话历史 →
/// 触发下一轮 LLM）：
/// - 联网：`web_search` / `fetch_url`
/// - 读：`read_file` / `list_dir` / `grep_files`
/// - 写：`write_file` / `edit_file`（限 workspace 内）
/// - 执行：`exec_shell` + 相关辅助
///
/// **透传给上层**（不在 auto_tool_names）：`request_user_input` —— harness 发出
/// `ToolCallStart` 事件，由 web/TUI 渲染选择卡。
pub fn build_default_tool_registry(
    workspace: &Path,
) -> (Arc<ToolRegistry>, ToolContext, HashSet<String>) {
    let workspace = workspace.to_path_buf();
    let notes_path = workspace.join(".deepseek").join("notes.md");
    let mcp_config_path = workspace.join(".deepseek").join("mcp.json");

    // YOLO 上下文：trust_mode=true（允许跨 workspace 读，但写仍限于 workspace），
    // auto_approve=true（不弹审批），让 LLM 调工具时直接跑。
    let context = ToolContext::with_auto_approve(
        workspace,
        true, // trust_mode
        notes_path,
        mcp_config_path,
        true, // auto_approve
    );

    let registry = ToolRegistryBuilder::new()
        .with_user_input_tool()
        .with_web_tools() // web_search / fetch_url / finance / web_run
        .with_file_tools() // read_file / write_file / edit_file / list_dir
        .with_search_tools() // grep_files / file_search
        .with_shell_tools() // exec_shell + wait/interact/cancel
        .build(context.clone());

    let mut auto = HashSet::new();
    // 联网
    auto.insert("web_search".to_string());
    auto.insert("fetch_url".to_string());
    // 读
    auto.insert("read_file".to_string());
    auto.insert("list_dir".to_string());
    auto.insert("grep_files".to_string());
    auto.insert("file_search".to_string());
    // 写（workspace 内）
    auto.insert("write_file".to_string());
    auto.insert("edit_file".to_string());
    // shell
    auto.insert("exec_shell".to_string());
    auto.insert("exec_shell_wait".to_string());
    auto.insert("exec_shell_interact".to_string());
    auto.insert("exec_shell_cancel".to_string());
    auto.insert("exec_wait".to_string());
    auto.insert("exec_interact".to_string());
    // request_user_input 不在自动列表 → 透传给上层

    (Arc::new(registry), context, auto)
}
