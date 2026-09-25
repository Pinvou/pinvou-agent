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
import {
  QUOTE_CHIP_ESTIMATED_WIDTH,
  QUOTE_CHIP_GAP,
  quoteChipPosition,
  resolveDismissedSelection,
  sameRangeDescriptor,
  selectionRangeDescriptor,
} from '../src/features/aux-chat/aux-quote-selection-state.mjs';
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
  // dedupe compares by exact identity (trailing-edge trim only) and does not add an entry
  const dup = addAuxQuote([{ text: 'hello  world' }], 'hello  world');
  assert.equal(dup.ok, true);
  assert.equal(dup.duplicate, true);
  assert.equal(dup.quotes.length, 1);
  const dupTrimmed = addAuxQuote([{ text: 'hello  world' }], '  hello  world  ');
  assert.equal(dupTrimmed.duplicate, true, 'edge whitespace still trims away');
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

test('quote identity is exact: case and line structure are semantic (round-30 D5)', () => {
  // The old whitespace/case-collapsing identity merged each of these pairs
  // into one quote and reported success — silently dropping a selection.
  const pairs = [
    ['const x = 1;', 'CONST X = 1;'],
    ['line1\nline2', 'line1 line2'],
    ['foo', 'Foo'],
    ['hello  world', 'hello world'],
  ];
  for (const [first, second] of pairs) {
    const result = addAuxQuote([{ text: first }], second);
    assert.equal(result.ok, true, `"${second}" must stage alongside "${first}"`);
    assert.equal(result.duplicate, false, `"${second}" is not a duplicate of "${first}"`);
    assert.equal(result.quotes.length, 2);
  }
  // staging store: distinct variants are kept separately, exact duplicates dedupe
  stageAuxQuote('task-exact', 'const x = 1;');
  const variant = stageAuxQuote('task-exact', 'CONST X = 1;');
  assert.equal(variant.duplicate, false);
  assert.equal(getAuxQuotes('task-exact').length, 2, 'case variants must coexist');
  const exact = stageAuxQuote('task-exact', 'const x = 1;');
  assert.equal(exact.ok, true);
  assert.equal(exact.duplicate, true, 'exact duplicates still dedupe');
  assert.equal(getAuxQuotes('task-exact').length, 2);
  dropAuxQuotes('task-exact', getAuxQuotes('task-exact'));
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

test('dropAuxQuotes matches by exact identity after an edge trim (round-30 D5)', () => {
  stageAuxQuote('task-norm', 'Hello  World');
  stageAuxQuote('task-norm', 'Another');
  // only the exact text (up to trailing-edge whitespace) matches; case or
  // internal-whitespace variants must NOT remove a staged quote
  dropAuxQuotes('task-norm', [{ text: ' Hello  World ' }]);
  assert.deepEqual(getAuxQuotes('task-norm'), [{ text: 'Another' }]);
  stageAuxQuote('task-norm', 'Keep Me');
  dropAuxQuotes('task-norm', [{ text: 'keep me' }]);
  dropAuxQuotes('task-norm', [{ text: 'Another ' }]);
  assert.deepEqual(
    getAuxQuotes('task-norm'),
    [{ text: 'Keep Me' }],
    'lowercase variant must not match; exact text with an edge trim does',
  );
  dropAuxQuotes('task-norm', getAuxQuotes('task-norm'));
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

// ── Selection popover decisions (aux-quote-selection-state.mjs) ──
//
// Fake selections: plain objects stand in for DOM nodes (identity comparison
// only), mirroring the aux-chat-state.mjs pure-helper test pattern.

const fakeSelection = ({ anchor = {}, focus = anchor, anchorOffset = 0, focusOffset = 5, collapsed = false } = {}) => ({
  rangeCount: 1,
  isCollapsed: collapsed,
  anchorNode: anchor,
  anchorOffset,
  focusNode: focus,
  focusOffset,
});

test('selectionRangeDescriptor captures endpoints and rejects collapsed/invalid selections', () => {
  assert.equal(selectionRangeDescriptor(null), null);
  assert.equal(selectionRangeDescriptor({ rangeCount: 0, isCollapsed: false }), null);
  assert.equal(selectionRangeDescriptor(fakeSelection({ collapsed: true })), null);
  assert.equal(selectionRangeDescriptor({ rangeCount: 1, isCollapsed: false, anchorNode: null, focusNode: {} }), null);
  const anchor = {};
  const focus = {};
  assert.deepEqual(
    selectionRangeDescriptor(fakeSelection({ anchor, focus, anchorOffset: 2, focusOffset: 9 })),
    { anchorNode: anchor, anchorOffset: 2, focusNode: focus, focusOffset: 9 },
  );
});

test('sameRangeDescriptor compares by node identity and offsets', () => {
  const shared = {};
  const a = { anchorNode: shared, anchorOffset: 0, focusNode: shared, focusOffset: 5 };
  assert.equal(sameRangeDescriptor(a, { ...a }), true);
  assert.equal(sameRangeDescriptor(a, { ...a, focusOffset: 6 }), false);
  // same shape, different node instances: a genuinely new range
  assert.equal(sameRangeDescriptor(a, { anchorNode: {}, anchorOffset: 0, focusNode: {}, focusOffset: 5 }), false);
  assert.equal(sameRangeDescriptor(null, a), false);
  assert.equal(sameRangeDescriptor(a, null), false);
});

test('Escape latch: the same keypress must not resurrect the dismissed popover (M2)', () => {
  // Sequence from the bug report: select text → chip appears → Escape keydown
  // latches the live range and hides → the same keypress's keyup re-evaluates
  // one macrotask later with the selection UNCHANGED (Escape does not
  // collapse a DOM selection).
  const selection = fakeSelection();
  const dismissed = selectionRangeDescriptor(selection);
  const afterEscapeKeyup = resolveDismissedSelection(dismissed, selectionRangeDescriptor(selection));
  assert.equal(afterEscapeKeyup.suppress, true, 'an unchanged selection must stay suppressed after Escape');
  assert.equal(afterEscapeKeyup.dismissed, dismissed, 'the latch survives while the selection is unchanged');
  // Any later keypress (e.g. Ctrl+C to copy) re-evaluates the same range and
  // must stay suppressed too.
  const afterCopy = resolveDismissedSelection(afterEscapeKeyup.dismissed, selectionRangeDescriptor(selection));
  assert.equal(afterCopy.suppress, true);
});

test('Escape latch clears when the selection genuinely changes or collapses (M2)', () => {
  const dismissed = selectionRangeDescriptor(fakeSelection());
  // Collapse (click elsewhere): latch clears and nothing is suppressed.
  const collapsed = resolveDismissedSelection(dismissed, null);
  assert.deepEqual(collapsed, { dismissed: null, suppress: false });
  // A new range (even over the same text with fresh node tokens) clears the
  // latch, so a deliberate re-selection raises the chip again.
  const reselected = resolveDismissedSelection(dismissed, selectionRangeDescriptor(fakeSelection()));
  assert.deepEqual(reselected, { dismissed: null, suppress: false });
  // Extending the same selection (shift+arrow changes the focus offset) is a
  // genuine change too.
  const anchor = {};
  const extended = resolveDismissedSelection(
    selectionRangeDescriptor(fakeSelection({ anchor, focusOffset: 5 })),
    selectionRangeDescriptor(fakeSelection({ anchor, focusOffset: 8 })),
  );
  assert.deepEqual(extended, { dismissed: null, suppress: false });
  // No latch → never suppress.
  assert.deepEqual(resolveDismissedSelection(null, selectionRangeDescriptor(fakeSelection())), {
    dismissed: null,
    suppress: false,
  });
});

test('quoteChipPosition clamps the chip inside the conversation column (M1)', () => {
  const containerRect = { left: 200, top: 100, width: 800, height: 2000 };
  // Centered over the selection, relative to the container.
  const centered = quoteChipPosition(containerRect, { left: 500, top: 400, width: 100, height: 20 });
  assert.equal(centered.left, 500 + 50 - QUOTE_CHIP_ESTIMATED_WIDTH / 2 - 200);
  assert.equal(centered.top, 400 - 100 - 36 - QUOTE_CHIP_GAP);
  // A selection hugging the right edge clamps to the column's inner right
  // edge — the chip can never extend past the column into the right dock.
  const rightEdge = quoteChipPosition(containerRect, { left: 950, top: 400, width: 40, height: 20 });
  assert.equal(rightEdge.left, 800 - QUOTE_CHIP_ESTIMATED_WIDTH - QUOTE_CHIP_GAP);
  // Left edge and top edge clamp to the gap.
  const leftEdge = quoteChipPosition(containerRect, { left: 200, top: 400, width: 10, height: 20 });
  assert.equal(leftEdge.left, QUOTE_CHIP_GAP);
  const topEdge = quoteChipPosition(containerRect, { left: 500, top: 104, width: 100, height: 20 });
  assert.equal(topEdge.top, QUOTE_CHIP_GAP);
});

test('quoteChipPosition falls back to a container-centered anchor for a zero rect', () => {
  // Hidden/suspended WebView: the selection is valid but layout reports a
  // zero-size rect; the quote action must survive at a clamped fallback.
  const containerRect = { left: 0, top: 0, width: 800, height: 2000 };
  const fallback = quoteChipPosition(containerRect, { left: 0, top: 0, width: 0, height: 0 });
  assert.equal(fallback.left, 800 / 2 - 120 / 2 + 120 / 2 - QUOTE_CHIP_ESTIMATED_WIDTH / 2);
  assert.equal(fallback.top, 120 - 36 - QUOTE_CHIP_GAP);
  const noRect = quoteChipPosition(containerRect, null);
  assert.deepEqual(noRect, fallback);
});
