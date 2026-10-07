# The main conversation remains the overall coordinator. The inherited
# EngineConfig(max_spawn_depth = 2) caps children at two levels. Since
# foundation #5253 a per-call max_depth can only NARROW the inherited cap
# (the engine clamps to min(inherited, child_depth + requested), where
# child_depth is the spawning child's own depth), so a
# positive override can no longer widen the ceiling; this hook still keeps
# the session cap authoritative in multi-agent sessions (the bundle-0.20
# decision: positive overrides are intercepted) and must state that truth.
# Attached only to multi-agent sessions.
#
# Deny contract: exit 2 + single-line stdout JSON {"decision":"deny",
# "reason":...}. The engine reads the reason only from stdout JSON; stderr
# never reaches the model.

$ErrorActionPreference = "Stop"

$toolName = [string]$env:DEEPSEEK_TOOL_NAME
$toolArgs = [string]$env:DEEPSEEK_TOOL_ARGS

$hasOpaqueWorkflowSource = $toolName -eq "workflow" -and [regex]::IsMatch(
    $toolArgs,
    '(?<!\\)"(source_path|path)"\s*:'
)

if ($hasOpaqueWorkflowSource) {
    [Console]::Out.WriteLine(
        '{"decision":"deny","reason":"Multi-agent mode requires the workflow source to be inlined in the call so child depth can be enforced; a source_path reference is rejected here. Inline the workflow source instead."}'
    )
    exit 2
}

$hasPositiveDepth = switch ($toolName) {
    "agent" {
        [regex]::IsMatch(
            $toolArgs,
            '(?<!\\)"(max_depth|maxDepth|max_spawn_depth)"\s*:\s*[1-9][0-9]*'
        )
        break
    }
    "workflow" {
        # Inline JS tasks may pass the depth override in camelCase (maxDepth)
        # or snake_case (max_depth — the TaskOptions serde alias).
        [regex]::IsMatch(
            $toolArgs,
            '(?<!\\)("max_depth"\s*:|(?<=[{,\s])max_depth\s*:|maxDepth\s*:)\s*[1-9][0-9]*'
        )
        break
    }
    default { $false }
}

if ($hasPositiveDepth) {
    [Console]::Out.WriteLine(
        '{"decision":"deny","reason":"Multi-agent mode caps children at two levels. A per-call max_depth can only narrow that cap and is rejected here to keep the session cap authoritative; omit max_depth to inherit the session limit, or set it to 0 for a leaf."}'
    )
    exit 2
}

exit 0
