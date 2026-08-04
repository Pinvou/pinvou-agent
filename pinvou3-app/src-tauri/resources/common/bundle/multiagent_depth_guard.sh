#!/usr/bin/env bash
# Multi-agent mode keeps orchestration in the root conversation.  CodeWhale's
# per-call maxDepth/max_depth option may widen the inherited runtime budget, so
# reject positive overrides and let EngineConfig(max_spawn_depth = 1) remain the
# authoritative ceiling.  This hook is attached only to multi-agent sessions.

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
    'Multi-agent mode allows direct child agents only. Remove the depth override or set it to 0.' \
    >&2
  exit 2
fi

exit 0
