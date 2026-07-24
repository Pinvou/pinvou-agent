const ESC = String.fromCharCode(0x1b);
const BEL = String.fromCharCode(0x07);
const C1_CSI = String.fromCharCode(0x9b);
const OSC_SEQUENCE = new RegExp(`${ESC}\\][\\s\\S]*?(?:${BEL}|${ESC}\\\\)`, 'g');
const CSI_SEQUENCE = new RegExp(`(?:${ESC}\\[|${C1_CSI})[0-?]*[ -/]*[@-~]`, 'g');
const SINGLE_ESCAPE_SEQUENCE = new RegExp(`${ESC}[()][0-2A-Z]`, 'g');
const OPERATION_ITEM_TYPES = new Set(['command_execution', 'file_change', 'tool']);
const SEARCH_TOOL_NAMES = new Set([
  'web_search',
  'mcp_iwencai_news_search',
  'search_web',
]);
const FETCH_TOOL_NAMES = new Set([
  'fetch_url',
  'web_fetch',
  'web.fetch',
]);

/**
 * 浏览器不是终端，展示命令和输出前清理 ANSI、OSC 超链接等控制序列。
 */
export function stripTerminalControlSequences(value) {
  return String(value ?? '')
    .replace(OSC_SEQUENCE, '')
    .replace(CSI_SEQUENCE, '')
    .replace(SINGLE_ESCAPE_SEQUENCE, '');
}

function searchToolName(tool) {
  return String(tool && (tool.name || tool.title) || '').trim().toLowerCase();
}

export function isSearchTool(tool) {
  const name = searchToolName(tool);
  return String(tool && tool.kind || '').toLowerCase() === 'search'
    || SEARCH_TOOL_NAMES.has(name);
}

export function isFetchTool(tool) {
  const name = searchToolName(tool);
  return String(tool && tool.kind || '').toLowerCase() === 'fetch'
    || FETCH_TOOL_NAMES.has(name);
}

function decodeSearchFragment(value) {
  return String(value || '')
    .replace(/\\r/g, '')
    .replace(/\\n/g, '\n')
    .replace(/\\"/g, '"')
    .replace(/\\\\/g, '\\')
    .trim();
}

function searchSourceLabel(name, rawOutput) {
  const sourceMatch = String(rawOutput || '').match(/"source"\s*:\s*"([^"]+)"/i);
  const source = sourceMatch && sourceMatch[1] ? sourceMatch[1].trim() : '';
  if (source) return source.toLowerCase() === 'bing' ? 'Bing' : source;
  if (name === 'mcp_iwencai_news_search') return '同花顺新闻';
  if (name === 'web_search') return '网页搜索';
  return '搜索';
}

function validSearchUrl(value) {
  const url = decodeSearchFragment(value);
  return /^https?:\/\/[^\s]+$/i.test(url) ? url : '';
}

function collectSearchResults(rawOutput) {
  const text = decodeSearchFragment(rawOutput);
  const results = [];
  const seen = new Set();
  function add(titleValue, urlValue) {
    const title = decodeSearchFragment(titleValue).replace(/\s+/g, ' ');
    const url = validSearchUrl(urlValue);
    if (!title || !url || seen.has(url)) return;
    seen.add(url);
    results.push({ title, url });
  }

  // web_search: title 紧邻 url；同花顺新闻 MCP: url 后夹带 id/uid 等字段再到 title。
  let match;
  const titleThenUrl = /"title"\s*:\s*"([^"\n]{1,500})"\s*,\s*"url"\s*:\s*"([^"\n]{1,2000})"/g;
  while ((match = titleThenUrl.exec(text))) add(match[1], match[2]);
  const urlThenTitle = /"url"\s*:\s*"(https?:[^"\n]{1,2000})"[\s\S]{0,700}?"title"\s*:\s*"([^"\n]{1,500})"/g;
  while ((match = urlThenTitle.exec(text))) add(match[2], match[1]);
  return results;
}

export function searchToolDetails(tool) {
  if (!isSearchTool(tool)) return null;
  const rawInput = tool && tool.rawInput && typeof tool.rawInput === 'object'
    ? tool.rawInput
    : {};
  const rawOutputValue = tool && (tool.rawOutput != null ? tool.rawOutput : tool.content);
  const rawOutput = typeof rawOutputValue === 'string'
    ? rawOutputValue
    : rawOutputValue == null
      ? ''
      : JSON.stringify(rawOutputValue, null, 2);
  const name = searchToolName(tool);
  const countMatch = rawOutput.match(/"count"\s*:\s*(\d+)/i);
  const query = String(rawInput.query || rawInput.q || rawInput.keyword || '').trim();
  const results = collectSearchResults(rawOutput);
  return {
    query,
    source: searchSourceLabel(name, rawOutput),
    count: countMatch ? Number(countMatch[1]) : null,
    results,
    rawOutput,
    compacted: /compacted to protect context|output truncated for context|\(Original:\s*\d+\s+chars/i.test(rawOutput),
  };
}

function fetchContentTypeLabel(contentType) {
  const normalized = String(contentType || '').toLowerCase();
  if (normalized.includes('html')) return 'HTML';
  if (normalized.includes('json')) return 'JSON';
  if (normalized.includes('markdown')) return 'Markdown';
  if (normalized.startsWith('text/')) return '文本';
  return normalized ? contentType : '网页内容';
}

export function fetchToolDetails(tool) {
  if (!isFetchTool(tool)) return null;
  const rawInput = tool && tool.rawInput && typeof tool.rawInput === 'object'
    ? tool.rawInput
    : {};
  const rawOutputValue = tool && (tool.rawOutput != null ? tool.rawOutput : tool.content);
  const rawOutput = typeof rawOutputValue === 'string'
    ? rawOutputValue
    : rawOutputValue == null
      ? ''
      : JSON.stringify(rawOutputValue, null, 2);
  let payload = rawOutputValue && typeof rawOutputValue === 'object' ? rawOutputValue : null;
  if (!payload && rawOutput) {
    try { payload = JSON.parse(rawOutput); } catch (_) {}
  }
  payload = payload && typeof payload === 'object' ? payload : {};
  const url = String(payload.url || rawInput.url || '').trim();
  let hostname = '';
  let target = url;
  try {
    const parsed = new URL(url);
    hostname = parsed.hostname.replace(/^www\./, '');
    const path = parsed.pathname === '/' ? '' : parsed.pathname;
    target = `${hostname}${path.length > 36 ? `${path.slice(0, 36)}…` : path}`;
  } catch (_) {}
  const statusValue = payload.status;
  const status = statusValue == null || statusValue === '' ? null : Number(statusValue);
  const content = typeof payload.content === 'string' ? payload.content : '';
  const preview = content.replace(/\s+/g, ' ').trim().slice(0, 320);
  const contentType = String(payload.content_type
    || (payload.headers && (payload.headers['content-type'] || payload.headers['Content-Type']))
    || '');
  return {
    url,
    hostname,
    target: target || hostname || '网页',
    status: Number.isFinite(status) ? status : null,
    contentType,
    contentTypeLabel: fetchContentTypeLabel(contentType),
    contentLength: content.length,
    preview,
    truncated: payload.truncated === true,
    rawOutput,
  };
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
