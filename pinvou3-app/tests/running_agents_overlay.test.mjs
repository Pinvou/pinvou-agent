/** 蜂群运行小窗纯模型（overlay-model.mjs）：可见性窗口 / 状态映射 / 缓存淘汰。 */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  MAX_OVERLAY_ENTRIES,
  RECENT_TERMINAL_MS,
  entryKey,
  isTerminal,
  overlayVisibleEntries,
  pruneOverlayEntries,
  statusPresentation,
} from '../src/features/multiagent/overlay-model.mjs';

const copy = {
  agentCard: { failed: '失败', completed: '已完成', working: '运行中' },
  blockedTag: '受阻',
  pendingTag: '等待中',
};

const entry = (overrides = {}) => ({
  sessionId: 's1',
  agentId: 'agent_1',
  done: false,
  failed: false,
  blocked: false,
  status: 'running',
  ...overrides,
});

test('isTerminal：done 且未受阻才是终态', () => {
  assert.equal(isTerminal(entry({ done: true })), true);
  assert.equal(isTerminal(entry({ done: true, blocked: true })), false, '受阻条目不算终态');
  assert.equal(isTerminal(entry({ done: false })), false);
  assert.equal(isTerminal(null), false);
});

test('overlayVisibleEntries：未终态进 active，刚完成的终态进 recent 窗口', () => {
  const now = 10_000;
  const entries = [
    entry({ agentId: 'agent_1' }),
    entry({ agentId: 'agent_2', done: true, completedAt: now - 1000 }),
    entry({ agentId: 'agent_3', done: true, completedAt: now - RECENT_TERMINAL_MS - 1 }),
    entry({ agentId: 'agent_4', done: true }),
  ];
  const { active, recent } = overlayVisibleEntries(entries, now);
  assert.deepEqual(active.map(item => item.agentId), ['agent_1']);
  assert.deepEqual(recent.map(item => item.agentId), ['agent_2'], '窗口外与无 completedAt 的终态不可见');
});

test('statusPresentation：终态优先；ledger 英文 token 映射 i18n，不裸露英文', () => {
  assert.equal(statusPresentation(entry({ done: true, failed: true }), copy).text, '失败');
  assert.equal(statusPresentation(entry({ done: true, blocked: true }), copy).dot, 'blocked');
  assert.equal(statusPresentation(entry({ done: true }), copy).text, '已完成');
  // ledger token（回归：'running' 曾被原样显示给 zh/ja 用户）。
  assert.equal(statusPresentation(entry({ status: 'running' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'RUNNING' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'queued' }), copy).text, '等待中');
  // 非白名单的单 token 视为实时进展短语，原样展示；空白短语回落到运行中。
  assert.equal(statusPresentation(entry({ status: 'reading files' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'scanning' }), copy).text, 'scanning');
  assert.equal(statusPresentation(entry({ status: null }), copy).text, '运行中');
});

test('entryKey：会话与 agentId 组合，空值安全', () => {
  assert.equal(entryKey('s1', 'agent_1'), 's1\u0000agent_1');
  assert.equal(entryKey(null, 'agent_1'), '\u0000agent_1');
});

test('pruneOverlayEntries：未超限返回 null，超限淘汰最老终态，保留非终态', () => {
  const small = { a: entry({ agentId: 'a' }) };
  assert.equal(pruneOverlayEntries(small, 2), null);

  const entries = {
    live: entry({ agentId: 'live' }),
    old: entry({ agentId: 'old', done: true, completedAt: 100 }),
    mid: entry({ agentId: 'mid', done: true, completedAt: 200 }),
    new: entry({ agentId: 'new', done: true, completedAt: 300 }),
  };
  const pruned = pruneOverlayEntries(entries, 3);
  assert.ok(pruned, '超限必须淘汰');
  assert.deepEqual(Object.keys(pruned).sort(), ['live', 'mid', 'new'], '最老终态先淘汰，非终态保留');
  // 输入不被原地修改。
  assert.deepEqual(Object.keys(entries).sort(), ['live', 'mid', 'new', 'old']);
});

test('pruneOverlayEntries：终态条目不足时返回 null（非终态不受影响）', () => {
  const entries = {};
  for (let i = 0; i < MAX_OVERLAY_ENTRIES + 5; i++) {
    entries[`agent_${i}`] = entry({ agentId: `agent_${i}` });
  }
  entries.done_one = entry({ agentId: 'done_one', done: true, completedAt: 1 });
  assert.equal(pruneOverlayEntries(entries), null, '唯一终态淘汰后仍超限：宁可不淘汰也不丢运行态');
});
