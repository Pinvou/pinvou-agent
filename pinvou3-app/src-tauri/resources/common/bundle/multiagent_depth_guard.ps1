# Multi-agent mode keeps orchestration in the root conversation. CodeWhale's
# per-call maxDepth/max_depth option may widen the inherited runtime budget, so
# reject positive overrides and let EngineConfig(max_spawn_depth = 1) remain the
# authoritative ceiling. This hook is attached only to multi-agent sessions.

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
        "Multi-agent mode allows direct child agents only. Remove the depth override or set it to 0."
    )
    exit 2
}

exit 0
