# The main conversation remains the overall coordinator. The inherited
# EngineConfig(max_spawn_depth = 2) permits one nested child layer, while a
# positive per-call maxDepth/max_depth override could widen that ceiling again
# at each level. Reject positive overrides and keep the session cap authoritative.
# This hook is attached only to multi-agent sessions.

$ErrorActionPreference = "Stop"

$toolName = [string]$env:DEEPSEEK_TOOL_NAME
$toolArgs = [string]$env:DEEPSEEK_TOOL_ARGS

$hasOpaqueWorkflowSource = $toolName -eq "workflow" -and [regex]::IsMatch(
    $toolArgs,
    '(?<!\\)"(source_path|path)"\s*:'
)

if ($hasOpaqueWorkflowSource) {
    [Console]::Error.WriteLine(
        "Multi-agent mode requires inline workflow script/plan input so child depth can be enforced; source_path is unavailable."
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
        [regex]::IsMatch(
            $toolArgs,
            '((?<!\\)"max_depth"\s*:|maxDepth\s*:)\s*[1-9][0-9]*'
        )
        break
    }
    default { $false }
}

if ($hasPositiveDepth) {
    [Console]::Error.WriteLine(
        "Multi-agent mode allows at most two child levels. Positive depth overrides can widen the inherited cap; omit max_depth to inherit the session limit, or set it to 0 for a leaf."
    )
    exit 2
}

exit 0
