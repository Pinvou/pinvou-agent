// 会话内后台 shell 任务指示器的纯逻辑：从 chatItems 派生当前会话仍在运行的
// 后台任务列表。抽成独立模块以便 node:test 直接单测
// （见 tests/background_tasks_logic.test.js）。

const COMMAND_SUMMARY_MAX = 80;

// 后台 shell 任务在 chatItems 里的形态（terminal.js 的 applyShellSnapshots 与
// markBackgroundToolItem 写入）：type='tool'、带 taskId、state='running'；
// 子 agent 启动的 detached job 由轮询补建 shellSnapshot 卡，同样命中该过滤。
function isRunningShellTaskItem(item) {
  return Boolean(item && item.type === 'tool' && item.taskId && item.state === 'running');
}

function summarizeCommand(command) {
  const text = String(command ?? '').trim();
  if (text.length <= COMMAND_SUMMARY_MAX) return text;
  return `${text.slice(0, COMMAND_SUMMARY_MAX)}…`;
}

// chatItems 是当前会话的工作集（切会话时整体换入换出），无需再按 sessionId 过滤。
function deriveRunningShellTasks(chatItems) {
  if (!Array.isArray(chatItems)) return [];
  return chatItems.filter(isRunningShellTaskItem).map(item => ({
    taskId: item.taskId,
    sessionId: item.sessionId,
    command: summarizeCommand(item.args && item.args.command),
    elapsedMs: typeof item.elapsedMs === 'number' ? item.elapsedMs : 0,
    output: typeof item.output === 'string' ? item.output : '',
  }));
}

// 耗时格式化：秒级以下显示秒，分钟级显示 "Xm Ys"，更长显示 "Xh Ym"。
function formatElapsedMs(ms) {
  const totalSeconds = Math.max(0, Math.floor((Number(ms) || 0) / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

// 输出 tail：取最后 n 行（空行不计），供指示器浮层展示。
function tailOutputLines(output, lineCount = 3) {
  const lines = String(output ?? '').split('\n').filter(line => line.trim() !== '');
  return lines.slice(-Math.max(1, lineCount)).join('\n');
}

export {
  COMMAND_SUMMARY_MAX,
  deriveRunningShellTasks,
  formatElapsedMs,
  isRunningShellTaskItem,
  tailOutputLines,
};
