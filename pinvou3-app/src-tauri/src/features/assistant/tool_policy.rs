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
///
/// Known taught-but-excluded residuals (disclosed, not oversights): the
/// base `agent` description offers the `Run` tool (`action="verifiers"`)
/// as an alternative to `type=test`, and the goal surfaces teach
/// `create_goal` / `update_goal`; none of them is admitted, so a model
/// following those names ends in the usual excluded-tool dead end. This is
/// accepted today because `type=test` is the primary route the same sentence
/// leads with, and the goal surfaces (bare `/goal` slash briefs and
/// goal-continuation prompts) are unreachable from this host — the GUI never
/// sets goals and slash briefs are not dispatched here. If either surface
/// becomes reachable, admit or gate the names in the same change: the same
/// phantom rule that drove the evidence-recovery admissions applies.
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
    // Snapshot-based turn rollback is dead in Pinvou parent sessions:
    // `snapshots_enabled` is false (app-side code_checkpoints owns rewind),
    // so every call fails with "0 pre-turn snapshots exist" after paying a
    // one-time base side-repo init. Keeping it admitted would offer the
    // model a permanently failing Required-approval tool, so it stays out
    // of the allowlist. (Subagent children inherit the full base registry
    // and are not gated by this list, so they still see the tool; that
    // asymmetry is foundation-level and stays with upstream.)
    "todo_write",
    // The spillover recovery contract: bash/Web overflow footers name
    // `retrieve_tool_result` directly and `evidence_available` metadata
    // auto-activates it next turn (base truncate.rs/web_run.rs/fetch_url.rs).
    // Excluding it used to turn every large-output spill into a 1-2 step
    // dead end (auto-activation rejected, tool_search scoped to the
    // retained-out catalog, direct call permission-denied). It is a bounded
    // read-only evidence reader — the same risk class as `read` — so it is
    // admitted for the teaching surfaces the base ships.
    "retrieve_tool_result",
    // Subagent worker records (roster/takeover/transcript projections) carry
    // HANDLE_READ_ACTIVATION_HINT teaching the parent to call `handle_read`
    // for a bounded view of a worker transcript. Excluding it silently
    // degraded swarm verification to at most two doomed probes per record.
    // The tool itself is a deferred read-only projection reader registered
    // unconditionally on the agent surface, so admission alone restores the
    // documented contract.
    "handle_read",
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
    // The ima connector's marketplace skill orders the model to call this
    // native tool directly, but it matches no family rule above (the `mcp_*`
    // prefix only covers MCP-discovered names), so without this entry the
    // per-turn catalog strip made the skill teach a permanently absent tool.
    // This admission is package-scope gated: the marketplace native-tool
    // ownership table emits `ima_openapi` into `disallowed_tools` whenever
    // the ima package is uninstalled or disabled for the session scope, and
    // deny wins over this admission (first-turn catalog, `tool_search`
    // results, and execution all reject).
    "ima_openapi",
    // Computer use stays in the allowlist unconditionally; while the feature
    // toggle is off the engine disallow list hides it and the consent guard
    // rejects every call. Referenced through the feature's canonical const
    // (not a literal) so a rename cannot silently strip the tool from every
    // model catalog with all suites green.
    crate::features::computer_use::TOOL_NAME,
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
/// - `computer_use`: the bridge renders the computer-use instruction section
///   (see `computer_use::instruction_block`) into every native session while
///   the master switch is on, and that section names the tool directly.
///   Deferring it would recreate exactly the failure the admission criterion
///   exists for — the prompt says the capability exists while the first-turn
///   tool list says it does not — and the `tool_search` recovery loses its
///   activation on engine respawn (observed 2026-09-30: discovery needed the
///   user to ask, then the tool vanished mid-task after an idle gap). While
///   the toggle is off the disallow list strips the tool anyway and the
///   section is not rendered, so both sides of visibility stay consistent.
///
/// Always-load only affects deferral and does not bypass `disallowed`
/// (deny wins).
pub const PINVOU3_ALWAYS_LOADED_TOOLS: &[&str] = &[
    // The shared instructions name this family directly. Deny rules still win.
    // Delivery is part of ordinary-mode completion; code-mode policy hides it.
    "mcp_pinvou3_present_artifact",
    "mcp_app-automations_create_scheduled_task",
    "mcp_app-automations_list_scheduled_tasks",
    "mcp_app-automations_read_scheduled_task",
    "mcp_app-automations_update_scheduled_task",
    "mcp_app-automations_delete_scheduled_task",
    "request_user_input",
    "image_analyze",
    "load_skill",
    "file_search",
    "registry_sync",
    "start_registry_mcp_server",
    crate::features::computer_use::TOOL_NAME,
];
#[must_use]
pub fn allowed_tool_names() -> Vec<String> {
    PINVOU3_ALLOWED_TOOLS
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

/// Test-only allowlist check. Production gating hands `allowed_tool_names()` to the
/// foundation, whose matcher additionally expands policy aliases; this local
/// reimplementation does not, so it is kept for the allowlist contract tests only and
/// must not be used as the production predicate.
#[must_use]
#[cfg(test)]
pub fn is_pinvou3_allowed(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    PINVOU3_ALLOWED_TOOLS.iter().any(|rule| {
        let rule = rule.to_ascii_lowercase();
        rule.strip_suffix('*')
            .map_or_else(|| name == rule, |prefix| name.starts_with(prefix))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn artifact_delivery_is_loaded_without_widening_code_mode_tools() {
        let name = "mcp_pinvou3_present_artifact";
        assert!(super::PINVOU3_ALWAYS_LOADED_TOOLS.contains(&name));
        assert!(super::is_pinvou3_allowed(name));
        let code = crate::features::assistant::session_policy::SessionPolicy::for_mode(
            crate::core::session_mode::SessionMode::Code,
        );
        assert!(code.unavailable_tools().contains(&name));
        let plain = crate::features::assistant::session_policy::SessionPolicy::for_mode(
            crate::core::session_mode::SessionMode::Plain,
        );
        assert!(!plain.unavailable_tools().contains(&name));
    }

    #[test]
    fn scheduled_family_is_available_without_tool_search() {
        for verb in [
            "create_scheduled_task",
            "read_scheduled_task",
            "list_scheduled_tasks",
            "update_scheduled_task",
            "delete_scheduled_task",
        ] {
            let name = format!("mcp_app-automations_{verb}");
            assert!(super::PINVOU3_ALWAYS_LOADED_TOOLS.contains(&name.as_str()));
            assert!(super::is_pinvou3_allowed(&name));
        }
    }

    /// Evidence-recovery teaching surfaces must stay reachable, and the
    /// snapshot-based rollback must stay out: the base spills overflow
    /// footers and worker-record hints that name these tools verbatim, so
    /// flipping either side recreates the "prompt says it exists, list says
    /// it does not" phantom this allowlist exists to prevent.
    #[test]
    fn evidence_recovery_is_admitted_and_snapshot_revert_stays_excluded() {
        assert!(super::is_pinvou3_allowed("retrieve_tool_result"));
        assert!(super::is_pinvou3_allowed("handle_read"));
        assert!(!super::is_pinvou3_allowed("revert_turn"));
        assert!(!super::PINVOU3_ALWAYS_LOADED_TOOLS.contains(&"retrieve_tool_result"));
        assert!(!super::PINVOU3_ALWAYS_LOADED_TOOLS.contains(&"handle_read"));
    }
}
