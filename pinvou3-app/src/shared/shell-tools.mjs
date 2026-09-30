// Single source of truth for the shell-execution tool-name set shared by the
// conversation projection (features/conversation) and the tool card renderers
// (features/tools). The platform bridge layers (tauri terminal.js, web
// bridge.js) keep their own copies because both ship as verbatim classic
// <script src> bundles and cannot import an ESM module; keep this list in
// sync with them when adding a new shell tool name (pinned by
// tests/shell_task_projection.test.mjs).
export const SHELL_TOOL_NAMES = new Set([
  'bash',
  'exec_shell',
  'exec_shell_wait',
  'exec_wait',
  'task_shell_start',
  'task_shell_wait',
  'shell',
  'Bash',
]);

export const isShellExecutionTool = name => SHELL_TOOL_NAMES.has(name);
