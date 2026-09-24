/** Conversation-quote (selected-text quote) contract: block build/parse round trip, limits, per-task staging store, and the aux turns projection. */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  AUX_QUOTE_LIMITS,
  addAuxQuote,
  buildAuxQuoteBlock,
  dropAuxQuotes,
  getAuxQuotes,
  parseAuxQuotedMessage,
  removeAuxQuote,
  stageAuxQuote,
  subscribeAuxQuotes,
} from '../src/features/aux-chat/aux-quote.mjs';
import { projectAuxChatTurns } from '../src/features/aux-chat/aux-chat-state.mjs';

test('buildAuxQuoteBlock and parseAuxQuotedMessage round trip consistently', () => {
  const quotes = [{ text: 'first quote' }, { text: 'line1\nline2 with ``` ticks' }];
  const block = buildAuxQuoteBlock(quotes);
  assert.ok(block.startsWith('\n\n# userselect:\n```userselect\n'), 'block header format is fixed');
  const { visibleText, quotes: parsed } = parseAuxQuotedMessage('what does this mean?' + block);
  assert.equal(visibleText, 'what does this mean?');
  assert.deepEqual(parsed, quotes);
});

test('buildAuxQuoteBlock returns an empty string for empty/invalid input', () => {
  assert.equal(buildAuxQuoteBlock([]), '');
  assert.equal(buildAuxQuoteBlock(null), '');
  assert.equal(buildAuxQuoteBlock([{ text: '   ' }, {}]), '');
});

test('parseAuxQuotedMessage strips only parseable blocks; bad JSON stays visible', () => {
  const malformed = 'question\n\n# userselect:\n```userselect\n[not json]\n```';
  const kept = parseAuxQuotedMessage(malformed);
  assert.equal(kept.quotes.length, 0);
  assert.equal(kept.visibleText, malformed.trim());
  const nonArray = '# userselect:\n```userselect\n{"text":"x"}\n```';
  const keptObject = parseAuxQuotedMessage(nonArray);
  assert.equal(keptObject.quotes.length, 0);
  assert.equal(keptObject.visibleText, nonArray);
});

test('parseAuxQuotedMessage tolerates CRLF and quote-only messages without body', () => {
  const block = buildAuxQuoteBlock([{ text: 'quote only' }]).replace(/\n/g, '\r\n');
  const { visibleText, quotes } = parseAuxQuotedMessage(block);
  assert.equal(visibleText, '');
  assert.equal(quotes.length, 1);
  assert.equal(quotes[0].text, 'quote only');
});

test('addAuxQuote enforces the single/count/total/dedupe limits', () => {
  assert.equal(addAuxQuote([], '   ').ok, false);
  assert.equal(addAuxQuote([], '   ').reason, 'empty');
  const tooLong = 'x'.repeat(AUX_QUOTE_LIMITS.single + 1);
  assert.equal(addAuxQuote([], tooLong).reason, 'single');
  // dedupe compares by whitespace normalization and does not add an entry
  const dup = addAuxQuote([{ text: 'hello  world' }], ' Hello\nWorld ');
  assert.equal(dup.ok, true);
  assert.equal(dup.duplicate, true);
  assert.equal(dup.quotes.length, 1);
  // count limit
  const eight = Array.from({ length: AUX_QUOTE_LIMITS.count }, (_, i) => ({ text: `q${i}` }));
  assert.equal(addAuxQuote(eight, 'one more').reason, 'count');
  // total limit: 8 entries approaching 16000 chars
  const near = Array.from({ length: 7 }, () => ({ text: 'x'.repeat(1990) }));
  const overflow = addAuxQuote(near, 'y'.repeat(AUX_QUOTE_LIMITS.single));
  assert.equal(overflow.reason, 'total');
  const fitting = addAuxQuote(near, 'y'.repeat(AUX_QUOTE_LIMITS.total - 7 * 1990));
  assert.equal(fitting.ok, true);
  assert.equal(fitting.quotes.length, 8);
});

test('staging store isolates per task and broadcasts changes', () => {
  const seen = [];
  const unsubscribe = subscribeAuxQuotes('task-a', (quotes) => seen.push(quotes));
  const stage = stageAuxQuote('task-a', 'first excerpt');
  assert.equal(stage.ok, true);
  assert.equal(getAuxQuotes('task-a').length, 1);
  assert.equal(getAuxQuotes('task-b').length, 0, 'tasks must not affect each other');
  assert.equal(seen.length, 1, 'a successful stage must broadcast');
  // duplicate content is neither re-staged nor broadcast
  stageAuxQuote('task-a', 'first excerpt');
  assert.equal(getAuxQuotes('task-a').length, 1);
  assert.equal(seen.length, 1);
  // over-limit content is neither stored nor broadcast
  const rejected = stageAuxQuote('task-a', 'x'.repeat(AUX_QUOTE_LIMITS.single + 1));
  assert.equal(rejected.ok, false);
  assert.equal(getAuxQuotes('task-a').length, 1);
  assert.equal(seen.length, 1);
  removeAuxQuote('task-a', 0);
  assert.equal(getAuxQuotes('task-a').length, 0);
  assert.equal(seen.length, 2, 'removal must broadcast');
  stageAuxQuote('task-a', 'another excerpt');
  dropAuxQuotes('task-a', getAuxQuotes('task-a'));
  assert.equal(getAuxQuotes('task-a').length, 0);
  assert.equal(seen.length, 4, 'batch removal by snapshot must broadcast (re-stage + removal after the earlier remove)');
  unsubscribe();
  stageAuxQuote('task-a', 'after unsubscribe');
  assert.equal(seen.length, 4, 'no more broadcasts after unsubscribe');
  dropAuxQuotes('task-a', getAuxQuotes('task-a'));
  // getAuxQuotes returns a defensive copy: external mutation must not pollute the store
  const copy = getAuxQuotes('task-a');
  copy.push({ text: 'injected' });
  assert.equal(getAuxQuotes('task-a').length, 0);
  dropAuxQuotes('task-b', [{ text: 'ghost' }]);
});

test('removeAuxQuote is a no-op for invalid indexes', () => {
  stageAuxQuote('task-idx', 'only');
  removeAuxQuote('task-idx', 5);
  removeAuxQuote('task-idx', -1);
  removeAuxQuote('task-idx', 'x');
  assert.equal(getAuxQuotes('task-idx').length, 1);
  dropAuxQuotes('task-idx', getAuxQuotes('task-idx'));
});

test('dropAuxQuotes removes only the send-time snapshot; quotes staged in flight are kept', () => {
  const seen = [];
  const unsubscribe = subscribeAuxQuotes('task-drop', (quotes) => seen.push(quotes));
  stageAuxQuote('task-drop', 'quote A');
  stageAuxQuote('task-drop', 'quote B');
  // the snapshot captured at send start contains only A: B was staged while the send was in flight
  dropAuxQuotes('task-drop', [{ text: 'quote A' }]);
  assert.deepEqual(getAuxQuotes('task-drop'), [{ text: 'quote B' }]);
  assert.equal(seen.length, 3, 'two stages + one removal broadcast three times in total');
  // snapshot matching no existing entry: no change, no broadcast (same as the duplicate-stage branch)
  dropAuxQuotes('task-drop', [{ text: 'quote A' }]);
  assert.equal(seen.length, 3, 'no change, no broadcast');
  unsubscribe();
  dropAuxQuotes('task-drop', getAuxQuotes('task-drop'));
});

test('dropAuxQuotes matches by whitespace/case normalization', () => {
  stageAuxQuote('task-norm', 'Hello  World');
  stageAuxQuote('task-norm', 'Another');
  // whitespace/case variants normalization-equivalent to 'Hello  World' also match
  dropAuxQuotes('task-norm', [{ text: ' hello\nworld ' }]);
  assert.deepEqual(getAuxQuotes('task-norm'), [{ text: 'Another' }]);
  dropAuxQuotes('task-norm', [{ text: 'ANOTHER' }]);
  assert.equal(getAuxQuotes('task-norm').length, 0, 'different case still matches; cleared to zero');
});

test('dropAuxQuotes is a no-op for empty taskId/missing task/empty input', () => {
  stageAuxQuote('task-safe', 'kept');
  dropAuxQuotes('', [{ text: 'kept' }]);
  dropAuxQuotes(null, [{ text: 'kept' }]);
  dropAuxQuotes('task-missing', [{ text: 'kept' }]);
  dropAuxQuotes('task-safe', []);
  dropAuxQuotes('task-safe', null);
  dropAuxQuotes('task-safe', [{ text: '   ' }]);
  assert.doesNotThrow(() => dropAuxQuotes());
  assert.deepEqual(getAuxQuotes('task-safe'), [{ text: 'kept' }]);
  dropAuxQuotes('task-safe', getAuxQuotes('task-safe'));
});

test('projectAuxChatTurns strips the quote block and attaches userQuotes', () => {
  const message = 'explain this for me' + buildAuxQuoteBlock([{ text: 'quoted content A' }, { text: 'quoted content B' }]);
  const snapshot = {
    chatItems: [
      { id: 1, type: 'user', text: message },
      { id: 2, type: 'assistant', text: 'explanation follows.' },
      { id: 3, type: 'user', text: 'a plain question without quotes' },
    ],
    busy: false,
    queued: [],
  };
  const turns = projectAuxChatTurns(snapshot, 'aux-q1');
  assert.equal(turns.length, 2);
  assert.equal(turns[0].userText, 'explain this for me');
  assert.deepEqual(turns[0].userQuotes, [{ text: 'quoted content A' }, { text: 'quoted content B' }]);
  assert.equal(turns[1].userText, 'a plain question without quotes');
  assert.equal(turns[1].userQuotes, undefined);
});

test('projectAuxChatTurns projects a quote-only message as empty body + quote chips', () => {
  const snapshot = {
    chatItems: [{ id: 1, type: 'user', text: buildAuxQuoteBlock([{ text: 'only a quote' }]).trim() }],
    busy: false,
    queued: [],
  };
  const turns = projectAuxChatTurns(snapshot, 'aux-q2');
  assert.equal(turns.length, 1);
  assert.equal(turns[0].userText, '');
  assert.deepEqual(turns[0].userQuotes, [{ text: 'only a quote' }]);
});
