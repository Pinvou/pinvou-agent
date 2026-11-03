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

# Opaque-source gate.  The engine also honors a top-level "path" as a
# manual fallback for source_path (an optional_str fallback inside
# workflow_source in workflow/mod.rs, not part of the tool schema), and
# matching that bare key hit ordinary nested "path" fields inside
# structured plan payloads (child tool args etc.), denying legitimate
# calls; the deny message then named only source_path.  The gate now
# triggers on any string-valued "source_path" key — grep cannot count
# JSON nesting depth, so a nested source_path in a free-form payload
# still denies (conservative, fail-closed) — and the message names
# exactly that key.  The "path" fallback is not intercepted: bare-path
# interception predates the engine's per-call max_depth min-clamp
# (#5253) and was load-bearing only before it landed; the clamp now
# bounds any override inside the file the engine loads, so this residual
# is bounded by the same engine rule this hook exists to state.
$hasOpaqueWorkflowSource = $toolName -eq "workflow" -and [regex]::IsMatch(
    $toolArgs,
    '(?<!\\)"source_path"\s*:\s*"'
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
        # TaskOptions (rename_all = camelCase) carries max_depth under the
        # canonical key "maxDepth" with the snake_case "max_depth" serde
        # alias — both arrive quoted in JSON payloads, and bare in inline JS.
        [regex]::IsMatch(
            $toolArgs,
            '(?<!\\)(("max_depth"|"maxDepth")\s*:|(?<=[{,\s])max_depth\s*:|maxDepth\s*:)\s*[1-9][0-9]*'
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
