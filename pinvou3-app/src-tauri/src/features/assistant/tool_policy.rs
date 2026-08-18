//! Pinvou 产品层的模型工具白名单。
//!
//! CodeWhale 0.9.5 以 canonical action family 作为模型侧工具面，并由
//! `allowed_tools` 同时约束首轮目录、`tool_search` 结果和实际执行。Pinvou 只在
//! 宿主层声明需要的家族与动态工具前缀，不在底座维护历史工具名黑名单。

/// Pinvou 对话允许进入模型工具目录的 canonical 工具与动态工具规则。
///
/// 规则语义与 CodeWhale `allowed_tools` 一致：名称大小写不敏感，尾部 `*`
/// 表示前缀匹配。MCP 的具体工具名由已启用连接器动态发现，因此只允许标准
/// `mcp_` 命名空间；连接器开关仍通过 `disallowed_tools` 施加更窄的拒绝规则。
///
/// 进度工具必须是 v0.9.5 canonical 模型可见名 `todo_write`；`work_update` /
/// `checklist_write` / `update_plan` 是隐藏的 replay 兼容别名（`model_visible()`
/// 为 false，不会出现在模型目录），写进白名单只是死条目。
pub const PINVOU3_ALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "File",
    "Git",
    "Web",
    "agent",
    "load_skill",
    "request_user_input",
    "revert_turn",
    "todo_write",
    "workflow",
    "tool_search",
    "image_analyze",
    "kb_search",
    "kb_open_source",
    "mcp_*",
    "list_mcp_resources",
    "list_mcp_resource_templates",
    "read_mcp_resource",
];

/// 需要首轮直接可见、不能依赖模型先调用 `tool_search` 的工具。
/// `agent` 是 PinvouOS Front→Orchestrator 的稳定控制面，若延迟发现，Front
/// 会在已经判断为复杂任务后反而找不到唯一后台入口。
pub const PINVOU3_ALWAYS_LOADED_TOOLS: &[&str] = &["agent", "request_user_input", "image_analyze"];

#[must_use]
pub fn allowed_tool_names() -> Vec<String> {
    PINVOU3_ALLOWED_TOOLS
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

#[must_use]
pub fn is_pinvou3_allowed(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    PINVOU3_ALLOWED_TOOLS.iter().any(|rule| {
        let rule = rule.to_ascii_lowercase();
        rule.strip_suffix('*')
            .map_or_else(|| name == rule, |prefix| name.starts_with(prefix))
    })
}
