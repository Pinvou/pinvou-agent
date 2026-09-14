//! Pinvou 产品层的模型工具白名单。
//!
//! CodeWhale 0.9.12 以 canonical model-visible tool name 作为模型侧工具面，并由
//! `allowed_tools` 同时约束首轮目录、`tool_search` 结果和实际执行。Pinvou 只在
//! 宿主层声明需要的家族与动态工具前缀，不在底座维护历史工具名黑名单。

/// Pinvou 对话允许进入模型工具目录的 canonical 工具与动态工具规则。
///
/// 规则语义与 CodeWhale `allowed_tools` 一致：名称大小写不敏感，尾部 `*`
/// 表示前缀匹配。MCP 的具体工具名由已启用连接器动态发现，因此只允许标准
/// `mcp_` 命名空间；连接器开关仍通过 `disallowed_tools` 施加更窄的拒绝规则。
///
/// 文件和前台 shell 已从 v0.9.x action family 迁到 `read` / `write` / `edit` /
/// `bash` 小契约；`File` / `Bash` 只保留为隐藏 replay 兼容名。持久终端的五个
/// 工具使用其精确名称，避免 `terminal/*` 让未来新增执行能力自动穿透产品白名单。
///
/// 进度工具必须是 v0.9.12 canonical 模型可见名 `todo_write`；`work_update` /
/// `checklist_write` / `update_plan` 是隐藏的 replay 兼容别名（`model_visible()`
/// 为 false，不会出现在模型目录），写进白名单只是死条目。
pub const PINVOU3_ALLOWED_TOOLS: &[&str] = &[
    "bash",
    "read",
    "write",
    "edit",
    "list_dir",
    "file_search",
    "grep_files",
    "Git",
    "Web",
    "terminal/run",
    "terminal/send",
    "terminal/wait",
    "terminal/cancel",
    "terminal/reset",
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
    // The two model-facing tools named by the base registry-first policy (the
    // runtime:mcp-registry-first instruction injected by Engine::new): the
    // instruction says "call these first", so the allowlist must admit them;
    // otherwise the model is ordered to call tools missing from its tool list
    // (observed as a broken reasoning loop in an exercise check-in Work-card
    // session).
    "registry_sync",
    "start_registry_mcp_server",
    "mcp_*",
    "list_mcp_resources",
    "list_mcp_resource_templates",
    "read_mcp_resource",
];

/// Tools that must be visible on the first turn and cannot rely on the model
/// first calling `tool_search`.
///
/// Admission criterion: tools that static instructions or base-injected
/// policies tell the model to call directly — if any of them is missing from
/// the first-turn tool list, the model gets stuck on "the description says it
/// exists, the list says it does not". The base first turn always carries
/// read/write/edit/bash/agent/todo_write; every other native tool is
/// deferred by default (`tool_search` stays active so the model can
/// re-activate deferred tools).
///
/// - `load_skill`: the Usage line of the base-rendered skills index teaches
///   the model to call `load_skill` directly.
/// - `file_search`: the work instructions teach the model to use it to find
///   user files.
/// - `registry_sync` / `start_registry_mcp_server`: the base registry-first
///   policy orders the model to call these two first.
///
/// Always-load only affects deferral and does not bypass `disallowed`
/// (deny wins).
pub const PINVOU3_ALWAYS_LOADED_TOOLS: &[&str] = &[
    "request_user_input",
    "image_analyze",
    "load_skill",
    "file_search",
    "registry_sync",
    "start_registry_mcp_server",
];
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
