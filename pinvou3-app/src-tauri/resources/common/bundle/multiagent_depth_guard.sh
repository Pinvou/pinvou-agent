#!/usr/bin/env bash
# The main conversation remains the overall coordinator.  The inherited
# EngineConfig(max_spawn_depth = 2) caps children at two levels.  Since
# foundation #5253 a per-call max_depth can only NARROW the inherited cap
# (the engine clamps to min(inherited, child_depth + requested), where
# child_depth is the spawning child's own depth), so a
# positive override can no longer widen the ceiling; this hook still keeps
# the session cap authoritative in multi-agent sessions (the bundle-0.20
# decision: positive overrides are intercepted) and must state that truth.
# This hook is attached only to multi-agent sessions.
#
# Deny contract: exit 2 + single-line stdout JSON {"decision":"deny",
# "reason":...}.  turn_loop.rs fold_tool_call_before_results takes the reason
# only from stdout JSON — stderr never reaches the model.

set -u

tool_name="${DEEPSEEK_TOOL_NAME:-}"
tool_args="${DEEPSEEK_TOOL_ARGS:-}"

if [ "$tool_name" = "workflow" ] &&
  printf '%s' "$tool_args" | grep -Eq '(^|[^\\])"(source_path|path)"[[:space:]]*:'; then
  printf '%s\n' \
    '{"decision":"deny","reason":"Multi-agent mode requires the workflow source to be inlined in the call so child depth can be enforced; a source_path reference is rejected here. Inline the workflow source instead."}'
  exit 2
fi

case "$tool_name" in
  agent)
    pattern='(^|[^\\])"(max_depth|maxDepth|max_spawn_depth)"[[:space:]]*:[[:space:]]*[1-9][0-9]*'
    ;;
  workflow)
    # Workflow accepts structured plans and inline JS tasks.  TaskOptions
    # (rename_all = camelCase) carries max_depth under the canonical key
    # "maxDepth" with the snake_case "max_depth" serde alias — both arrive
    # quoted in JSON payloads, and bare in inline JS.  The multi-agent
    # prompt does not recommend Workflow, but the same ceiling still
    # applies when the model chooses that existing tool.
    pattern='(^|[^\\])("(max_depth|maxDepth)"[[:space:]]*:|[{,[:space:]]max_depth[[:space:]]*:|maxDepth[[:space:]]*:)[[:space:]]*[1-9][0-9]*'
    ;;
  *)
    exit 0
    ;;
esac

if printf '%s' "$tool_args" | grep -Eq "$pattern"; then
  printf '%s\n' \
    '{"decision":"deny","reason":"Multi-agent mode caps children at two levels. A per-call max_depth can only narrow that cap and is rejected here to keep the session cap authoritative; omit max_depth to inherit the session limit, or set it to 0 for a leaf."}'
  exit 2
fi

exit 0
