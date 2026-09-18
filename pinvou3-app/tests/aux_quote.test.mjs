/** Conversation-quote ("划词引用") contract: block build/parse round trip, limits, per-task staging store, and the aux turns projection. */
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

test('buildAuxQuoteBlock 与 parseAuxQuotedMessage 往返一致', () => {
  const quotes = [{ text: '第一段引用' }, { text: 'line1\nline2 with ``` ticks' }];
  const block = buildAuxQuoteBlock(quotes);
  assert.ok(block.startsWith('\n\n# userselect:\n```userselect\n'), '块头部格式固定');
  const { visibleText, quotes: parsed } = parseAuxQuotedMessage('这是什么意思？' + block);
  assert.equal(visibleText, '这是什么意思？');
  assert.deepEqual(parsed, quotes);
});

test('buildAuxQuoteBlock 对空/无效输入返回空串', () => {
  assert.equal(buildAuxQuoteBlock([]), '');
  assert.equal(buildAuxQuoteBlock(null), '');
  assert.equal(buildAuxQuoteBlock([{ text: '   ' }, {}]), '');
});

test('parseAuxQuotedMessage 只剥离可解析的块，坏 JSON 保持可见', () => {
  const malformed = '问题\n\n# userselect:\n```userselect\n[not json]\n```';
  const kept = parseAuxQuotedMessage(malformed);
  assert.equal(kept.quotes.length, 0);
  assert.equal(kept.visibleText, malformed.trim());
  const nonArray = '# userselect:\n```userselect\n{"text":"x"}\n```';
  const keptObject = parseAuxQuotedMessage(nonArray);
  assert.equal(keptObject.quotes.length, 0);
  assert.equal(keptObject.visibleText, nonArray);
});

test('parseAuxQuotedMessage 容忍 CRLF 与仅引用无正文', () => {
  const block = buildAuxQuoteBlock([{ text: '仅引用' }]).replace(/\n/g, '\r\n');
  const { visibleText, quotes } = parseAuxQuotedMessage(block);
  assert.equal(visibleText, '');
  assert.equal(quotes.length, 1);
  assert.equal(quotes[0].text, '仅引用');
});

test('addAuxQuote 执行单条/条数/总量/去重限额', () => {
  assert.equal(addAuxQuote([], '   ').ok, false);
  assert.equal(addAuxQuote([], '   ').reason, 'empty');
  const tooLong = 'x'.repeat(AUX_QUOTE_LIMITS.single + 1);
  assert.equal(addAuxQuote([], tooLong).reason, 'single');
  // 去重按空白归一化比较,不算重复条数
  const dup = addAuxQuote([{ text: 'hello  world' }], ' Hello\nWorld ');
  assert.equal(dup.ok, true);
  assert.equal(dup.duplicate, true);
  assert.equal(dup.quotes.length, 1);
  // 条数上限
  const eight = Array.from({ length: AUX_QUOTE_LIMITS.count }, (_, i) => ({ text: `q${i}` }));
  assert.equal(addAuxQuote(eight, 'one more').reason, 'count');
  // 总量上限:8 条逼近 16000 字符
  const near = Array.from({ length: 7 }, () => ({ text: 'x'.repeat(1990) }));
  const overflow = addAuxQuote(near, 'y'.repeat(AUX_QUOTE_LIMITS.single));
  assert.equal(overflow.reason, 'total');
  const fitting = addAuxQuote(near, 'y'.repeat(AUX_QUOTE_LIMITS.total - 7 * 1990));
  assert.equal(fitting.ok, true);
  assert.equal(fitting.quotes.length, 8);
});

test('暂存 store 按任务隔离并广播变更', () => {
  const seen = [];
  const unsubscribe = subscribeAuxQuotes('task-a', (quotes) => seen.push(quotes));
  const stage = stageAuxQuote('task-a', '第一段');
  assert.equal(stage.ok, true);
  assert.equal(getAuxQuotes('task-a').length, 1);
  assert.equal(getAuxQuotes('task-b').length, 0, '任务之间互不影响');
  assert.equal(seen.length, 1, '暂存成功要广播');
  // 重复内容不重复暂存也不广播
  stageAuxQuote('task-a', '第一段');
  assert.equal(getAuxQuotes('task-a').length, 1);
  assert.equal(seen.length, 1);
  // 超限内容不落库不广播
  const rejected = stageAuxQuote('task-a', 'x'.repeat(AUX_QUOTE_LIMITS.single + 1));
  assert.equal(rejected.ok, false);
  assert.equal(getAuxQuotes('task-a').length, 1);
  assert.equal(seen.length, 1);
  removeAuxQuote('task-a', 0);
  assert.equal(getAuxQuotes('task-a').length, 0);
  assert.equal(seen.length, 2, '移除要广播');
  stageAuxQuote('task-a', '再一段');
  dropAuxQuotes('task-a', getAuxQuotes('task-a'));
  assert.equal(getAuxQuotes('task-a').length, 0);
  assert.equal(seen.length, 4, '按快照整批移除要广播(移除后暂存+移除)');
  unsubscribe();
  stageAuxQuote('task-a', '退订后');
  assert.equal(seen.length, 4, '退订后不再广播');
  dropAuxQuotes('task-a', getAuxQuotes('task-a'));
  // getAuxQuotes 的返回是防御性拷贝:外部修改不污染 store
  const copy = getAuxQuotes('task-a');
  copy.push({ text: 'injected' });
  assert.equal(getAuxQuotes('task-a').length, 0);
  dropAuxQuotes('task-b', [{ text: 'ghost' }]);
});

test('removeAuxQuote 对非法下标是 no-op', () => {
  stageAuxQuote('task-idx', 'only');
  removeAuxQuote('task-idx', 5);
  removeAuxQuote('task-idx', -1);
  removeAuxQuote('task-idx', 'x');
  assert.equal(getAuxQuotes('task-idx').length, 1);
  dropAuxQuotes('task-idx', getAuxQuotes('task-idx'));
});

test('dropAuxQuotes 只移除发送开始时的快照，在途新暂存保留', () => {
  const seen = [];
  const unsubscribe = subscribeAuxQuotes('task-drop', (quotes) => seen.push(quotes));
  stageAuxQuote('task-drop', '引用 A');
  stageAuxQuote('task-drop', '引用 B');
  // 发送开始时捕获的快照只含 A:B 是发送在途期间新暂存的
  dropAuxQuotes('task-drop', [{ text: '引用 A' }]);
  assert.deepEqual(getAuxQuotes('task-drop'), [{ text: '引用 B' }]);
  assert.equal(seen.length, 3, '两次暂存+一次移除共广播三次');
  // 快照未命中任何现存条目:无变化不广播(与重复暂存分支一致)
  dropAuxQuotes('task-drop', [{ text: '引用 A' }]);
  assert.equal(seen.length, 3, '无变化不广播');
  unsubscribe();
  dropAuxQuotes('task-drop', getAuxQuotes('task-drop'));
});

test('dropAuxQuotes 按空白/大小写归一化命中', () => {
  stageAuxQuote('task-norm', 'Hello  World');
  stageAuxQuote('task-norm', 'Another');
  // 与 'Hello  World' 归一化等价的空白/大小写变体也能命中
  dropAuxQuotes('task-norm', [{ text: ' hello\nworld ' }]);
  assert.deepEqual(getAuxQuotes('task-norm'), [{ text: 'Another' }]);
  dropAuxQuotes('task-norm', [{ text: 'ANOTHER' }]);
  assert.equal(getAuxQuotes('task-norm').length, 0, '大小写不同仍命中,清到零');
});

test('dropAuxQuotes 空 taskId/不存在任务/空入参是 no-op', () => {
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

test('projectAuxChatTurns 剥离引用块并挂 userQuotes', () => {
  const message = '帮我解释这段' + buildAuxQuoteBlock([{ text: '引用内容 A' }, { text: '引用内容 B' }]);
  const snapshot = {
    chatItems: [
      { id: 1, type: 'user', text: message },
      { id: 2, type: 'assistant', text: '解释如下。' },
      { id: 3, type: 'user', text: '没有引用的普通问题' },
    ],
    busy: false,
    queued: [],
  };
  const turns = projectAuxChatTurns(snapshot, 'aux-q1');
  assert.equal(turns.length, 2);
  assert.equal(turns[0].userText, '帮我解释这段');
  assert.deepEqual(turns[0].userQuotes, [{ text: '引用内容 A' }, { text: '引用内容 B' }]);
  assert.equal(turns[1].userText, '没有引用的普通问题');
  assert.equal(turns[1].userQuotes, undefined);
});

test('projectAuxChatTurns 仅引用消息投影为空正文 + 引用 chips', () => {
  const snapshot = {
    chatItems: [{ id: 1, type: 'user', text: buildAuxQuoteBlock([{ text: '只有引用' }]).trim() }],
    busy: false,
    queued: [],
  };
  const turns = projectAuxChatTurns(snapshot, 'aux-q2');
  assert.equal(turns.length, 1);
  assert.equal(turns[0].userText, '');
  assert.deepEqual(turns[0].userQuotes, [{ text: '只有引用' }]);
});
