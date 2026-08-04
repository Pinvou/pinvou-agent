import { scanMarkdownFences } from '../../shared/markdown-fences.js';

export function normalizeAssistantMessageText(value) {
  const lines = String(value || '')
    .replace(/\r\n?/g, '\n')
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

function fencePayload(body) {
  const source = String(body || '').trim();
  return source.startsWith('{') ? parseLooseJson(source) : null;
}

function selectStructuredFence(blocks, selected, serializer, explicitLanguage, allowGeneric) {
  let fallback = null;
  for (const block of blocks) {
    if (selected.has(block.index)) continue;
    const copyText = serializer(block.payload);
    if (!copyText) continue;
    if (block.language.includes(explicitLanguage)) return { block, copyText };
    if (allowGeneric && !fallback) fallback = { block, copyText };
  }
  return fallback;
}

function structuredFenceSelections(blocks, { allowScheduledTaskDraft = false } = {}) {
  const selected = new Map();
  const persona = selectStructuredFence(blocks, selected, personaCopyText, 'persona-card', true);
  if (persona) selected.set(persona.block.index, persona.copyText);
  if (allowScheduledTaskDraft) {
    const scheduled = selectStructuredFence(
      blocks,
      selected,
      scheduledTaskCopyText,
      'scheduled-task-draft',
      true,
    );
    if (scheduled) selected.set(scheduled.block.index, scheduled.copyText);
  }
  const question = selectStructuredFence(blocks, selected, questionCopyText, 'card-question', false);
  if (question) selected.set(question.block.index, question.copyText);
  return selected;
}

/**
 * Preserve the assistant's Markdown as the canonical copy format while replacing
 * machine-facing structured payloads with the same semantic content shown by cards.
 */
export function assistantMarkdownCopyText(value, options) {
  const markdown = normalizeAssistantMessageText(value);
  if (!markdown) return '';
  const fences = scanMarkdownFences(markdown);
  const blocks = fences.map((fence, index) => ({
    index,
    language: String(fence.info || '').trim().toLowerCase(),
    payload: fencePayload(fence.content),
  }));
  const selections = structuredFenceSelections(blocks, options);
  let cursor = 0;
  const output = [];
  fences.forEach((fence, index) => {
    output.push(markdown.slice(cursor, fence.start));
    output.push(selections.get(index) || markdown.slice(fence.start, fence.end));
    cursor = fence.end;
  });
  output.push(markdown.slice(cursor));
  const readable = output.join('');
  return normalizeAssistantMessageText(readable);
}
