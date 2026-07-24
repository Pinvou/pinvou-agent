const ESC = String.fromCharCode(0x1b);
const BEL = String.fromCharCode(0x07);
const C1_CSI = String.fromCharCode(0x9b);
const OSC_SEQUENCE = new RegExp(`${ESC}\\][\\s\\S]*?(?:${BEL}|${ESC}\\\\)`, 'g');
const CSI_SEQUENCE = new RegExp(`(?:${ESC}\\[|${C1_CSI})[0-?]*[ -/]*[@-~]`, 'g');
const SINGLE_ESCAPE_SEQUENCE = new RegExp(`${ESC}[()][0-2A-Z]`, 'g');
const OPERATION_ITEM_TYPES = new Set(['command_execution', 'file_change', 'tool']);

/**
 * 浏览器不是终端，展示命令和输出前清理 ANSI、OSC 超链接等控制序列。
 */
export function stripTerminalControlSequences(value) {
  return String(value ?? '')
    .replace(OSC_SEQUENCE, '')
    .replace(CSI_SEQUENCE, '')
    .replace(SINGLE_ESCAPE_SEQUENCE, '');
}

/**
 * Item 是事实语义，presentation 只控制视觉聚合。工具组不会改写、合并或丢弃
 * 任何 Item；展开后仍按原始时序逐项展示。
 */
export function presentConversationItems(items) {
  const result = [];
  for (const item of items || []) {
    if (OPERATION_ITEM_TYPES.has(item.type)) {
      const previous = result[result.length - 1];
      if (previous && previous.type === 'tool_group') {
        previous.items.push(item);
        continue;
      }
      result.push({
        id: `tool-group-${item.id}`,
        type: 'tool_group',
        items: [item],
      });
      continue;
    }
    result.push(item);
  }
  return result;
}

export function commandExecutionDetails(tool) {
  const rawInput = tool && tool.rawInput;
  const rawOutput = tool && tool.rawOutput;
  const command = rawInput && typeof rawInput === 'object' && rawInput.command != null
    ? stripTerminalControlSequences(rawInput.command)
    : stripTerminalControlSequences(tool && tool.title || '');
  const cwd = rawInput && typeof rawInput === 'object' && rawInput.cwd != null
    ? stripTerminalControlSequences(rawInput.cwd)
    : '';
  let output = '';
  let exitCode = null;
  if (typeof rawOutput === 'string') {
    output = rawOutput;
  } else if (rawOutput && typeof rawOutput === 'object') {
    output = String(
      rawOutput.formatted_output
        ?? rawOutput.output
        ?? rawOutput.text
        ?? '',
    );
    const code = rawOutput.exit_code ?? rawOutput.exitCode;
    if (code !== undefined && code !== null && code !== '') exitCode = Number(code);
  }
  if (!output && tool && typeof tool.content === 'string') output = tool.content;
  output = stripTerminalControlSequences(output);
  const commandLines = command.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  return {
    command,
    cwd,
    output,
    exitCode: Number.isNaN(exitCode) ? null : exitCode,
    summary: commandLines[0] || String(tool && tool.title || '执行 Shell 命令'),
    commandCount: commandLines.length,
  };
}

export function timestampMs(value) {
  if (typeof value === 'number') return Number.isFinite(value) ? value : NaN;
  const parsed = Date.parse(value || '');
  return Number.isFinite(parsed) ? parsed : NaN;
}

export function elapsedMs(start, end, now = Date.now()) {
  const from = timestampMs(start);
  const parsedEnd = timestampMs(end);
  const to = Number.isFinite(parsedEnd) ? parsedEnd : now;
  if (!Number.isFinite(from) || !Number.isFinite(to)) return 0;
  return Math.max(0, to - from);
}

export function formatElapsed(milliseconds) {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  if (seconds < 60) return `${seconds}秒`;
  const minutes = Math.floor(seconds / 60);
  const remaining = seconds % 60;
  return remaining ? `${minutes}分${remaining}秒` : `${minutes}分`;
}

export function terminalStatus(status, exitCode = null) {
  const normalized = String(status || '').toLowerCase();
  if (normalized === 'failed' || (exitCode != null && exitCode !== 0)) return 'failed';
  if (['completed', 'done', 'cancelled', 'canceled'].includes(normalized)) return 'completed';
  return 'running';
}
