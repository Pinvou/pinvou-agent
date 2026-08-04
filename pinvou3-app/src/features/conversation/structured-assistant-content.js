export function normalizeAssistantMessageText(value) {
  const lines = String(value || '')
    .replace(/\r\n?/g, '\n')
    .replace(/\u00a0/g, ' ')
    .split('\n');
  while (lines.length && /^[ \t]*$/.test(lines[0])) lines.shift();
  while (lines.length && /^[ \t]*$/.test(lines[lines.length - 1])) lines.pop();
  let normalized = lines.join('\n');
  if (!/^(?: {4}|\t)/.test(normalized)) normalized = normalized.replace(/^[ \t]+/, '');
  return normalized.replace(/[ \t]+$/, '');
}

export function extractBalancedJson(value) {
  const source = String(value || '');
  const start = source.indexOf('{');
  if (start < 0) return null;
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let index = start; index < source.length; index += 1) {
    const char = source.charAt(index);
    if (inString) {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === '"') inString = false;
    } else if (char === '"') inString = true;
    else if (char === '{') depth += 1;
    else if (char === '}') {
      depth -= 1;
      if (depth === 0) return source.slice(start, index + 1);
    }
  }
  return null;
}

export function parseJsonChain(value) {
  const source = String(value || '');
  try { return JSON.parse(source); } catch {}
  try { return JSON.parse(source.replace(/,(\s*[}\]])/g, '$1')); } catch {}
  const balanced = extractBalancedJson(source);
  if (balanced) {
    try { return JSON.parse(balanced); } catch {}
    try { return JSON.parse(balanced.replace(/,(\s*[}\]])/g, '$1')); } catch {}
  }
  return null;
}

export function parseLooseJson(value) {
  const source = String(value || '');
  const parsed = parseJsonChain(source);
  if (parsed) return parsed;
  const unescaped = source.replace(/\\"/g, '"');
  return unescaped !== source ? parseJsonChain(unescaped) : null;
}

function questionCopyText(payload) {
  if (!payload?.question || !Array.isArray(payload.options)) return '';
  const options = payload.options
    .filter(option => typeof option === 'string' && option.trim())
    .map((option, index) => `${index + 1}. ${option.trim()}`);
  if (!options.length) return '';
  return `${String(payload.question).trim()}\n\n${options.join('\n')}`;
}

function personaCopyText(payload) {
  if (!payload?.name || !payload?.body) return '';
  const title = [payload.emoji, payload.name]
    .filter(value => typeof value === 'string' && value.trim())
    .map(value => value.trim())
    .join(' ');
  const summary = typeof payload.description === 'string' && payload.description.trim()
    ? payload.description.trim()
    : typeof payload.dept === 'string'
      ? payload.dept.trim()
      : '';
  return [title, summary].filter(Boolean).join('\n\n');
}

function scheduledTaskCopyText(payload) {
  if (!payload?.name || !payload?.prompt || !payload?.rrule) return '';
  return [payload.name, payload.prompt, payload.rrule]
    .map(value => String(value).trim())
    .filter(Boolean)
    .join('\n\n');
}

function structuredFenceCopyText(info, body, { allowScheduledTaskDraft = false } = {}) {
  const payload = parseLooseJson(String(body || '').trim());
  if (!payload) return '';
  const language = String(info || '').trim().toLowerCase();
  if (language.includes('card-question')) return questionCopyText(payload);
  if (language.includes('scheduled-task-draft')) {
    return allowScheduledTaskDraft ? scheduledTaskCopyText(payload) : '';
  }
  if (language.includes('persona-card')) return personaCopyText(payload);
  return personaCopyText(payload)
    || (allowScheduledTaskDraft ? scheduledTaskCopyText(payload) : '');
}

/**
 * Preserve the assistant's Markdown as the canonical copy format while replacing
 * machine-facing structured payloads with the same semantic content shown by cards.
 */
export function assistantMarkdownCopyText(value, options) {
  const markdown = normalizeAssistantMessageText(value);
  if (!markdown) return '';
  const fencePattern = /(^|\n)(`{3,}|~{3,})([^\n]*)\n([\s\S]*?)\n\2(?=\n|$)/g;
  const readable = markdown.replace(fencePattern, (match, prefix, _fence, info, body) => {
    const structured = structuredFenceCopyText(info, body, options);
    return structured ? `${prefix}${structured}` : match;
  });
  return normalizeAssistantMessageText(readable);
}
