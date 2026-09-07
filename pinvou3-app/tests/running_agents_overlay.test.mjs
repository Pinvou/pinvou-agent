/** 蜂群运行小窗纯模型（overlay-model.mjs）：可见性窗口 / 状态映射 / 缓存淘汰 / 条目合并。 */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  MAX_OVERLAY_ENTRIES,
  RECENT_TERMINAL_MS,
  entryKey,
  isTerminal,
  mergeOverlayEntry,
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

const byCodePoint = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

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
  assert.deepEqual(Object.keys(pruned).sort(byCodePoint), ['live', 'mid', 'new'], '最老终态先淘汰，非终态保留');
  // 输入不被原地修改。
  assert.deepEqual(Object.keys(entries).sort(byCodePoint), ['live', 'mid', 'new', 'old']);
});

test('pruneOverlayEntries：终态条目不足时返回 null（非终态不受影响）', () => {
  const entries = {};
  for (let i = 0; i < MAX_OVERLAY_ENTRIES + 5; i++) {
    entries[`agent_${i}`] = entry({ agentId: `agent_${i}` });
  }
  entries.done_one = entry({ agentId: 'done_one', done: true, completedAt: 1 });
  assert.equal(pruneOverlayEntries(entries), null, '唯一终态淘汰后仍超限：宁可不淘汰也不丢运行态');
});

const ledgerRead = (overrides = {}) => ({
  sessionId: 's1',
  agentId: 'agent_1',
  role: null,
  status: 'completed',
  done: true,
  failed: false,
  blocked: false,
  source: 'ledger',
  ...overrides,
});

test('mergeOverlayEntry：冷启动快照不授予 completedAt，历史终态不进成功态窗口', () => {
  const now = 10_000;
  const merged = mergeOverlayEntry(null, ledgerRead(), 's1', now);
  assert.equal(merged.completedAt, undefined, '首次观测到的终态条目从未在本会话展示过运行态');
  assert.deepEqual(overlayVisibleEntries([merged], now).recent, [], '打开历史会话不得弹出「运行中 0」假胶囊');
});

test('mergeOverlayEntry：本会话内 运行→终态 翻转授予 completedAt', () => {
  const now = 10_000;
  const running = mergeOverlayEntry(null, ledgerRead({ done: false, status: 'running' }), 's1', now);
  assert.equal(running.completedAt, undefined);
  const done = mergeOverlayEntry(running, ledgerRead(), 's1', now);
  assert.equal(done.completedAt, now);
  assert.deepEqual(overlayVisibleEntries([done], now).recent.map(item => item.agentId), ['agent_1']);
  // 已是终态的迟到重复读数不重置计时起点。
  const again = mergeOverlayEntry(done, ledgerRead(), 's1', now + 1000);
  assert.equal(again.completedAt, now);
});

test('mergeOverlayEntry：终态 ratchet 拒绝迟到的非终态实时事件（返回 null）', () => {
  const done = mergeOverlayEntry(null, ledgerRead(), 's1', 10_000);
  done.completedAt = 10_000;
  assert.equal(
    mergeOverlayEntry(done, { done: false, status: 'still working', source: 'realtime' }, 's1', 11_000),
    null,
    '落盘终态是权威，非 ledger 的翻回不可变更',
  );
});

test('mergeOverlayEntry：ledger 非终态读数负责翻回运行中，completedAt 清除', () => {
  const done = mergeOverlayEntry(null, ledgerRead(), 's1', 10_000);
  const revived = mergeOverlayEntry(done, ledgerRead({ done: false, status: 'running' }), 's1', 11_000);
  assert.equal(revived.done, false);
  assert.equal(revived.completedAt, undefined, '落盘重唤醒后回到运行态，成功态窗口作废');
  assert.deepEqual(overlayVisibleEntries([revived], 11_000).active.map(item => item.agentId), ['agent_1']);
});

test('mergeOverlayEntry：受阻→解除按解除时刻授予成功态窗口', () => {
  const now = 10_000;
  // 受阻条目 done=true/blocked=true，不是终态。
  const coldBlocked = mergeOverlayEntry(null, ledgerRead({ done: true, blocked: true, status: 'waiting_input' }), 's1', now);
  assert.equal(coldBlocked.completedAt, undefined, '冷启动读到的受阻条目同样不授予');
  const unblocked = mergeOverlayEntry(coldBlocked, ledgerRead({ done: true, blocked: false, status: 'completed' }), 's1', now + 500);
  assert.equal(unblocked.completedAt, now + 500, '解除受阻才算真实完成翻转');
  assert.deepEqual(overlayVisibleEntries([unblocked], now + 500).recent.map(item => item.agentId), ['agent_1']);
});

test('mergeOverlayEntry：条目归属跟随传入会话，跨会话事件不串台', () => {
  const merged = mergeOverlayEntry(null, ledgerRead({ sessionId: 'other' }), 's1', 10_000);
  assert.equal(merged.sessionId, 's1', 'detail 缺失或错带 sessionId 时以订阅会话为准');
});
