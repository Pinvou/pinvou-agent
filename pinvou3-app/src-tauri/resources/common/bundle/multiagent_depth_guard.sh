#!/usr/bin/env bash
# The main conversation remains the overall coordinator.  The inherited
# EngineConfig(max_spawn_depth = 2) permits one nested child layer, while a
# positive per-call maxDepth/max_depth override could widen that ceiling again
# at each level.  Reject positive overrides and keep the session cap authoritative.
# This hook is attached only to multi-agent sessions.

set -u

tool_name="${DEEPSEEK_TOOL_NAME:-}"
tool_args="${DEEPSEEK_TOOL_ARGS:-}"

if [ "$tool_name" = "workflow" ] &&
  printf '%s' "$tool_args" | grep -Eq '(^|[^\\])"(source_path|path)"[[:space:]]*:'; then
  printf '%s\n' \
    'Multi-agent mode requires inline workflow script/plan input so child depth can be enforced; source_path is unavailable.' \
    >&2
  exit 2
fi

case "$tool_name" in
  agent)
    pattern='(^|[^\\])"(max_depth|maxDepth|max_spawn_depth)"[[:space:]]*:[[:space:]]*[1-9][0-9]*'
    ;;
  workflow)
    # Workflow accepts structured plans (max_depth) and inline JS tasks
    # (maxDepth).  The multi-agent prompt does not recommend Workflow, but the
    # same ceiling still applies when the model chooses that existing tool.
    pattern='(^|[^\\])("max_depth"[[:space:]]*:|maxDepth[[:space:]]*:)[[:space:]]*[1-9][0-9]*'
    ;;
  *)
    exit 0
    ;;
esac

if printf '%s' "$tool_args" | grep -Eq "$pattern"; then
  printf '%s\n' \
    'Multi-agent mode allows at most two child levels. Positive depth overrides can widen the inherited cap; omit max_depth to inherit the session limit, or set it to 0 for a leaf.' \
    >&2
  exit 2
fi

exit 0
