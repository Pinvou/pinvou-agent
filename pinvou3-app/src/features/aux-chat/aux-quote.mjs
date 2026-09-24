/**
 * Conversation quotes (selected-text quoting) for the aux chat — pure logic plus a
 * module-scoped per-task staging store.
 *
 * Closing the loop with the main conversation works the same way as the
 * upstream reference implementation: the user selects text in the main
 * conversation, the excerpt is staged as a pending quote for that task, and
 * on send the quotes travel inline with the aux message as a fenced
 * `userselect` block. The engine stays completely unaware of the feature —
 * it just receives a longer user message — and the timeline parses the block
 * back out so the panel renders quote chips instead of raw JSON.
 *
 * The aux session still never reads the main session's transcript by itself:
 * only excerpts the user explicitly selected are pushed, so the ADR-0006
 * isolation promise ("never reads or writes the main task's execution or
 * context" — no session-level
 * coupling) keeps its meaning; the pushed excerpt is user-mediated input,
 * the same as pasting the text by hand.
 */

export const AUX_QUOTE_LIMITS = Object.freeze({
  single: 8000,
  count: 8,
  total: 16000,
});

const QUOTE_BLOCK_HEADER = '# userselect:';
const QUOTE_BLOCK_FENCE = 'userselect';

// The block is emitted at the end of the aux message. The pattern tolerates
// \r\n and repeated blocks (defensive: only one is ever emitted), and only
// strips a block whose body parses as a JSON array — a hand-typed lookalike
// with malformed JSON stays visible instead of silently eating text.
const QUOTE_BLOCK_PATTERN = /(?:^|\r?\n)# userselect:\r?\n```userselect\r?\n([\s\S]*?)\r?\n```(?:\r?\n|$)/g;

// Identity is EXACT after a trailing-edge trim (round-30 D5): no case
// folding, no internal-whitespace collapse. In a coding agent both are
// semantic — `const x = 1;` vs `CONST X = 1;` and `line1\nline2` vs
// `line1 line2` are different quotes, and collapsing them silently dropped
// a selection the user believed was staged. Exact duplicates still dedupe.
function quoteIdentity(text) {
  return String(text || '').trim();
}

/**
 * Append the staged quotes to an aux message as one fenced block.
 *
 * @param {{ text: string }[]} quotes - staged quotes
 * @returns {string} the suffix to append (empty when there is nothing to send)
 */
export function buildAuxQuoteBlock(quotes) {
  const list = (Array.isArray(quotes) ? quotes : [])
    .map((quote) => (quote && typeof quote.text === 'string' ? quote.text.trim() : ''))
    .filter((text) => text.length > 0)
    .map((text) => ({ text }));
  if (!list.length) return '';
  return `\n\n${QUOTE_BLOCK_HEADER}\n\`\`\`${QUOTE_BLOCK_FENCE}\n${JSON.stringify(list)}\n\`\`\``;
}

/**
 * Split an aux message back into visible text and quoted excerpts.
 *
 * @param {string} text - raw aux message text
 * @returns {{ visibleText: string, quotes: { text: string }[] }}
 */
export function parseAuxQuotedMessage(text) {
  const raw = String(text || '');
  if (!raw) return { visibleText: '', quotes: [] };
  const quotes = [];
  const visible = raw.replace(QUOTE_BLOCK_PATTERN, (match, body) => {
    try {
      const parsed = JSON.parse(body);
      if (!Array.isArray(parsed)) return match;
      for (const entry of parsed) {
        if (entry && typeof entry === 'object' && typeof entry.text === 'string' && entry.text.trim()) {
          quotes.push({ text: entry.text });
        }
      }
      return '';
    } catch {
      return match;
    }
  });
  return { visibleText: visible.trim(), quotes };
}

/**
 * Validate and append one selection to the staged quotes.
 *
 * @param {{ text: string }[]} existingQuotes - already staged quotes
 * @param {string} text - the selected excerpt
 * @returns {{ ok: boolean, duplicate?: boolean, reason?: 'empty'|'single'|'count'|'total', quotes: { text: string }[] }}
 */
export function addAuxQuote(existingQuotes, text) {
  const trimmed = String(text || '').trim();
  const quotes = Array.isArray(existingQuotes) ? existingQuotes.slice() : [];
  if (!trimmed) return { ok: false, reason: 'empty', quotes };
  if (trimmed.length > AUX_QUOTE_LIMITS.single) return { ok: false, reason: 'single', quotes };
  const identity = quoteIdentity(trimmed);
  if (quotes.some((quote) => quoteIdentity(quote.text) === identity)) {
    return { ok: true, duplicate: true, quotes };
  }
  if (quotes.length >= AUX_QUOTE_LIMITS.count) return { ok: false, reason: 'count', quotes };
  const total = quotes.reduce((sum, quote) => sum + String(quote.text || '').length, 0);
  if (total + trimmed.length > AUX_QUOTE_LIMITS.total) return { ok: false, reason: 'total', quotes };
  quotes.push({ text: trimmed });
  return { ok: true, duplicate: false, quotes };
}

// Per-task pending quotes, module-scoped for the same reason as the panel's
// draftByTask: the quote was selected in the main conversation while the
// panel may be closed; it belongs to the task, not to the panel instance,
// and must survive panel close/reopen and task switches.
const pendingQuotesByTask = new Map();
const quoteListenersByTask = new Map();

function publishQuotes(taskId, quotes) {
  const listeners = quoteListenersByTask.get(taskId);
  if (!listeners) return;
  for (const listener of listeners) listener(quotes);
}

/**
 * Read the staged quotes for a task (defensive copy).
 *
 * @param {string} taskId - main session id
 * @returns {{ text: string }[]}
 */
export function getAuxQuotes(taskId) {
  const key = String(taskId || '');
  if (!key) return [];
  return (pendingQuotesByTask.get(key) || []).map((quote) => ({ text: quote.text }));
}

/**
 * Stage one selection for a task.
 *
 * @param {string} taskId - main session id
 * @param {string} text - selected excerpt
 * @returns {{ ok: boolean, duplicate?: boolean, reason?: string, quotes: { text: string }[] }}
 */
export function stageAuxQuote(taskId, text) {
  const key = String(taskId || '');
  const result = addAuxQuote(getAuxQuotes(key), text);
  if (result.ok && !result.duplicate && key) {
    pendingQuotesByTask.set(key, result.quotes.map((quote) => ({ text: quote.text })));
    publishQuotes(key, getAuxQuotes(key));
  }
  return result;
}

/**
 * Drop one staged quote by index.
 *
 * @param {string} taskId - main session id
 * @param {number} index
 */
export function removeAuxQuote(taskId, index) {
  const key = String(taskId || '');
  const quotes = getAuxQuotes(key);
  if (!Number.isInteger(index) || index < 0 || index >= quotes.length) return;
  quotes.splice(index, 1);
  if (quotes.length) pendingQuotesByTask.set(key, quotes);
  else pendingQuotesByTask.delete(key);
  publishQuotes(key, getAuxQuotes(key));
}

/**
 * Drop the staged quotes that traveled with a sent message.
 *
 * The send only owns the quotes captured when it started: quotes staged from
 * the main view while the send was in flight belong to the next message and
 * must survive the success callback. Entries are matched by quoteIdentity
 * (exact text after a trailing-edge trim — case and line structure are
 * semantic, round-30 D5); the staged entries and the send-time capture both
 * passed through the same staging trim, so the snapshot lands exactly, and
 * every staged entry whose identity matches is removed. Nothing matching
 * means no change and no broadcast.
 *
 * @param {string} taskId - main session id
 * @param {{ text: string }[]} quotesToRemove - staged-quote snapshot taken at send time
 */
export function dropAuxQuotes(taskId, quotesToRemove) {
  const key = String(taskId || '');
  const identities = new Set(
    (Array.isArray(quotesToRemove) ? quotesToRemove : [])
      .map((quote) => quoteIdentity(quote && quote.text))
      .filter((identity) => identity.length > 0),
  );
  if (!key || !identities.size) return;
  const current = getAuxQuotes(key);
  const kept = current.filter((quote) => !identities.has(quoteIdentity(quote.text)));
  if (kept.length === current.length) return;
  if (kept.length) pendingQuotesByTask.set(key, kept);
  else pendingQuotesByTask.delete(key);
  publishQuotes(key, getAuxQuotes(key));
}

/**
 * Subscribe to staged-quote changes for one task.
 *
 * @param {string} taskId - main session id
 * @param {(quotes: { text: string }[]) => void} listener
 * @returns {() => void} unsubscribe
 */
export function subscribeAuxQuotes(taskId, listener) {
  const key = String(taskId || '');
  if (!key || typeof listener !== 'function') return () => {};
  let listeners = quoteListenersByTask.get(key);
  if (!listeners) {
    listeners = new Set();
    quoteListenersByTask.set(key, listeners);
  }
  listeners.add(listener);
  return () => {
    const current = quoteListenersByTask.get(key);
    if (!current) return;
    current.delete(listener);
    if (!current.size) quoteListenersByTask.delete(key);
  };
}
