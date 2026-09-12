/** Pure model of the swarm running overlay (overlay-model.mjs): visibility window / status mapping / cache eviction / entry merge. */
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

// Fixture mirrors the zh locale's uiMultiAgent copy: statusPresentation must
// map ledger tokens into whatever localized copy it is given.
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

test('isTerminal: only done and unblocked is terminal', () => {
  assert.equal(isTerminal(entry({ done: true })), true);
  assert.equal(isTerminal(entry({ done: true, blocked: true })), false, 'a blocked entry is not terminal');
  assert.equal(isTerminal(entry({ done: false })), false);
  assert.equal(isTerminal(null), false);
});

test('overlayVisibleEntries: non-terminal goes active, just-finished terminal goes into the recent window', () => {
  const now = 10_000;
  const entries = [
    entry({ agentId: 'agent_1' }),
    entry({ agentId: 'agent_2', done: true, completedAt: now - 1000 }),
    entry({ agentId: 'agent_3', done: true, completedAt: now - RECENT_TERMINAL_MS - 1 }),
    entry({ agentId: 'agent_4', done: true }),
  ];
  const { active, recent } = overlayVisibleEntries(entries, now);
  assert.deepEqual(active.map(item => item.agentId), ['agent_1']);
  assert.deepEqual(recent.map(item => item.agentId), ['agent_2'], 'out-of-window and completedAt-less terminal entries are invisible');
});

test('statusPresentation: terminal first; ledger English tokens map to i18n copy, never shown raw', () => {
  assert.equal(statusPresentation(entry({ done: true, failed: true }), copy).text, '失败');
  assert.equal(statusPresentation(entry({ done: true, blocked: true }), copy).dot, 'blocked');
  assert.equal(statusPresentation(entry({ done: true }), copy).text, '已完成');
  // Ledger token (regression: 'running' used to be shown verbatim to zh/ja users).
  assert.equal(statusPresentation(entry({ status: 'running' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'RUNNING' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'queued' }), copy).text, '等待中');
  // A non-whitelisted single token is treated as a real-time progress phrase
  // and shown verbatim; a blank phrase falls back to working.
  assert.equal(statusPresentation(entry({ status: 'reading files' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'scanning' }), copy).text, 'scanning');
  assert.equal(statusPresentation(entry({ status: null }), copy).text, '运行中');
});

test('entryKey: combines session and agentId, null-safe', () => {
  assert.equal(entryKey('s1', 'agent_1'), 's1\u0000agent_1');
  assert.equal(entryKey(null, 'agent_1'), '\u0000agent_1');
});

test('pruneOverlayEntries: null when under the cap; past it evict the oldest terminal, keep non-terminal', () => {
  const small = { a: entry({ agentId: 'a' }) };
  assert.equal(pruneOverlayEntries(small, 2), null);

  const entries = {
    live: entry({ agentId: 'live' }),
    old: entry({ agentId: 'old', done: true, completedAt: 100 }),
    mid: entry({ agentId: 'mid', done: true, completedAt: 200 }),
    new: entry({ agentId: 'new', done: true, completedAt: 300 }),
  };
  const pruned = pruneOverlayEntries(entries, 3);
  assert.ok(pruned, 'over the cap, eviction is mandatory');
  assert.deepEqual(Object.keys(pruned).sort(byCodePoint), ['live', 'mid', 'new'], 'oldest terminal evicted first, non-terminal kept');
  // The input is never mutated in place.
  assert.deepEqual(Object.keys(entries).sort(byCodePoint), ['live', 'mid', 'new', 'old']);
});

test('pruneOverlayEntries: null when there are not enough terminal entries (non-terminal untouched)', () => {
  const entries = {};
  for (let i = 0; i < MAX_OVERLAY_ENTRIES + 5; i++) {
    entries[`agent_${i}`] = entry({ agentId: `agent_${i}` });
  }
  entries.done_one = entry({ agentId: 'done_one', done: true, completedAt: 1 });
  assert.equal(pruneOverlayEntries(entries), null, 'still over the cap after evicting the only terminal entry: rather keep than drop running state');
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

test('mergeOverlayEntry: cold-start snapshot grants no completedAt; historical terminal stays out of the success window', () => {
  const now = 10_000;
  const merged = mergeOverlayEntry(null, ledgerRead(), 's1', now);
  assert.equal(merged.completedAt, undefined, 'a terminal entry first observed here was never shown running in this session');
  assert.deepEqual(overlayVisibleEntries([merged], now).recent, [], 'opening a historical session must not flash a fake "running 0" pill');
});

test('mergeOverlayEntry: a running → terminal flip within this session grants completedAt', () => {
  const now = 10_000;
  const running = mergeOverlayEntry(null, ledgerRead({ done: false, status: 'running' }), 's1', now);
  assert.equal(running.completedAt, undefined);
  const done = mergeOverlayEntry(running, ledgerRead(), 's1', now);
  assert.equal(done.completedAt, now);
  assert.deepEqual(overlayVisibleEntries([done], now).recent.map(item => item.agentId), ['agent_1']);
  // A late duplicate terminal reading does not reset the window start.
  const again = mergeOverlayEntry(done, ledgerRead(), 's1', now + 1000);
  assert.equal(again.completedAt, now);
});

test('mergeOverlayEntry: terminal ratchet rejects a late non-terminal real-time event (returns null)', () => {
  const done = mergeOverlayEntry(null, ledgerRead(), 's1', 10_000);
  done.completedAt = 10_000;
  assert.equal(
    mergeOverlayEntry(done, { done: false, status: 'still working', source: 'realtime' }, 's1', 11_000),
    null,
    'the persisted terminal state is authoritative; non-ledger flip-backs cannot change it',
  );
});

test('mergeOverlayEntry: a ledger non-terminal reading flips back to running and clears completedAt', () => {
  const done = mergeOverlayEntry(null, ledgerRead(), 's1', 10_000);
  const revived = mergeOverlayEntry(done, ledgerRead({ done: false, status: 'running' }), 's1', 11_000);
  assert.equal(revived.done, false);
  assert.equal(revived.completedAt, undefined, 're-awakened from the ledger means running again; the success window is void');
  assert.deepEqual(overlayVisibleEntries([revived], 11_000).active.map(item => item.agentId), ['agent_1']);
});

test('mergeOverlayEntry: unblock grants the success window at the moment of unblocking', () => {
  const now = 10_000;
  // A blocked entry has done=true/blocked=true and is not terminal.
  const coldBlocked = mergeOverlayEntry(null, ledgerRead({ done: true, blocked: true, status: 'waiting_input' }), 's1', now);
  assert.equal(coldBlocked.completedAt, undefined, 'a blocked entry read cold-start gets no grant either');
  const unblocked = mergeOverlayEntry(coldBlocked, ledgerRead({ done: true, blocked: false, status: 'completed' }), 's1', now + 500);
  assert.equal(unblocked.completedAt, now + 500, 'only clearing the block counts as a real completion flip');
  assert.deepEqual(overlayVisibleEntries([unblocked], now + 500).recent.map(item => item.agentId), ['agent_1']);
});

test('mergeOverlayEntry: entry ownership follows the passed-in session; cross-session events do not leak', () => {
  const merged = mergeOverlayEntry(null, ledgerRead({ sessionId: 'other' }), 's1', 10_000);
  assert.equal(merged.sessionId, 's1', 'when detail lacks or carries a mismatched sessionId, the subscribing session wins');
});
