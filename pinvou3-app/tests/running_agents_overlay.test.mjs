/** Pure model of the swarm running overlay (overlay-model.mjs): visibility window / status mapping / cache eviction / entry merge. */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import {
  MAX_OVERLAY_ENTRIES,
  RECENT_TERMINAL_MS,
  entryKey,
  isTerminal,
  isUnknownLedgerRow,
  mergeOverlayEntry,
  overlayVisibleEntries,
  pruneOverlayEntries,
  statusPresentation,
} from '../src/features/multiagent/overlay-model.mjs';

// Source pin target: the component glue (session-switch cache discard,
// revival kick, ledger summary mapping) has no React test harness in this
// repo, so its wiring is pinned with the same source-regex convention as
// chat_turn_error_isolation.
const overlaySource = fs.readFileSync(
  new URL('../src/features/multiagent/RunningAgentsOverlay.jsx', import.meta.url),
  'utf8',
);

// Fixture mirrors the zh locale's uiMultiAgent copy: statusPresentation must
// map ledger tokens into whatever localized copy it is given.
const copy = {
  agentCard: { failed: '失败', completed: '已完成', working: '运行中', interrupted: '已中断', cancelled: '已取消' },
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

test('isTerminal: done is terminal; blocked is terminal too (authority chain), displayed separately', () => {
  assert.equal(isTerminal(entry({ done: true })), true);
  assert.equal(
    isTerminal(entry({ done: true, blocked: true })),
    true,
    'the foundation counts a blocked worker Completed; keeping it non-terminal would lock the poll at the active cadence',
  );
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

test('overlayVisibleEntries: a blocked entry stays listed (awaiting user) without riding the success window', () => {
  const now = 10_000;
  const blocked = entry({ agentId: 'agent_1', done: true, blocked: true });
  const { active, recent } = overlayVisibleEntries([blocked], now);
  assert.deepEqual(
    active.map(item => item.agentId),
    ['agent_1'],
    'a blocked entry is the waiting-on-the-user surface and must stay visible',
  );
  assert.deepEqual(recent, []);
  // It is terminal for the poll cadence: hasActive in the component keys off
  // isTerminal, so this shape must not hold the poll at the active rate.
  assert.equal(isTerminal(blocked), true);
});

test('statusPresentation: terminal first; ledger English tokens map to i18n copy, never shown raw', () => {
  assert.equal(statusPresentation(entry({ done: true, failed: true }), copy).text, '失败');
  assert.equal(statusPresentation(entry({ done: true, blocked: true }), copy).dot, 'blocked');
  assert.equal(statusPresentation(entry({ done: true }), copy).text, '已完成');
  // Ledger token (regression: 'running' used to be shown verbatim to zh/ja users).
  assert.equal(statusPresentation(entry({ status: 'running' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'RUNNING' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'queued' }), copy).text, '等待中');
  // The remaining ledger tokens must not leak raw snake_case either: a worker
  // executes tools under running_tool, throttles under model_wait, and parks
  // at a resumable checkpoint under waiting_for_user.
  assert.equal(statusPresentation(entry({ status: 'running_tool' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'model_wait' }), copy).text, '等待中');
  assert.equal(statusPresentation(entry({ status: 'waiting_for_user' }), copy).text, '等待中');
  // A non-whitelisted single token is treated as a real-time progress phrase
  // and shown verbatim; a blank phrase falls back to working.
  assert.equal(statusPresentation(entry({ status: 'reading files' }), copy).text, '运行中');
  assert.equal(statusPresentation(entry({ status: 'scanning' }), copy).text, 'scanning');
  assert.equal(statusPresentation(entry({ status: null }), copy).text, '运行中');
});

test('statusPresentation: a cancelled or interrupted terminal is not a dispatch failure', () => {
  // The ledger folds every non-completed ending into failed=true, but the
  // status token still distinguishes them: turning swarm off cancels live
  // children and a session restart interrupts them — neither is the agent's
  // failure, so both get their own copy and a neutral dot.
  assert.deepEqual(
    statusPresentation(entry({ done: true, failed: true, status: 'cancelled' }), copy),
    { text: '已取消', dot: 'stopped' },
  );
  assert.deepEqual(
    statusPresentation(entry({ done: true, failed: true, status: 'INTERRUPTED' }), copy),
    { text: '已中断', dot: 'stopped' },
  );
  assert.equal(statusPresentation(entry({ done: true, failed: true, status: 'failed' }), copy).dot, 'failed');
  assert.equal(
    statusPresentation(entry({ done: true, failed: true, status: null }), copy).text,
    '失败',
    'a failure without a distinguishing token stays a dispatch failure',
  );
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

test('pruneOverlayEntries: exactly enough terminal entries evicts back to the cap', () => {
  const entries = {
    live_a: entry({ agentId: 'live_a' }),
    live_b: entry({ agentId: 'live_b' }),
    live_c: entry({ agentId: 'live_c' }),
    old: entry({ agentId: 'old', done: true, completedAt: 100 }),
    mid: entry({ agentId: 'mid', done: true, completedAt: 200 }),
  };
  const pruned = pruneOverlayEntries(entries, 3);
  assert.ok(pruned, 'two terminal entries are exactly the overflow: evicting both reaches the cap');
  assert.deepEqual(Object.keys(pruned).sort(byCodePoint), ['live_a', 'live_b', 'live_c']);
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

test('isUnknownLedgerRow: orphan transcripts (done=false, no status token) are not live agents', () => {
  // transcripts.rs projects transcripts whose worker-ledger record was pruned
  // (foundation 256-record cap) as done=false/status=null unknown rows; the
  // overlay must skip them instead of showing eternal "working" ghosts.
  assert.equal(isUnknownLedgerRow({ agent_id: 'a', done: false, failed: false, blocked: false, status: null }), true);
  assert.equal(isUnknownLedgerRow({ agent_id: 'a', done: false, status: 'running' }), false, 'a real non-terminal reading carries a token');
  assert.equal(isUnknownLedgerRow({ agent_id: 'a', done: true, status: null }), false, 'a done reading is terminal regardless of the token');
  assert.equal(isUnknownLedgerRow(null), false);
});

test('mergeOverlayEntry: a status-less real-time completion keeps the ledger cancelled/interrupted token', () => {
  // The bridge cannot distinguish endings (agent_complete sends status:null),
  // but the ledger already recorded an operator cancellation: the spread must
  // not whiten it into a green "completed" until the next ledger read.
  const cancelled = mergeOverlayEntry(null, ledgerRead({ done: true, failed: true, status: 'cancelled' }), 's1', 10_000);
  const whitened = mergeOverlayEntry(
    cancelled,
    { sessionId: 's1', agentId: 'agent_1', done: true, failed: false, status: null, source: 'realtime' },
    's1',
    11_000,
  );
  assert.equal(whitened.status, 'cancelled', 'the distinguishing ledger token must survive');
  assert.equal(whitened.failed, true, 'the distinguishing failed flag must survive with it');
  assert.deepEqual(statusPresentation(whitened, copy), { text: '已取消', dot: 'stopped' });
  const interrupted = mergeOverlayEntry(
    mergeOverlayEntry(null, ledgerRead({ done: true, failed: true, status: 'interrupted' }), 's1', 10_000),
    { sessionId: 's1', agentId: 'agent_1', done: true, failed: false, status: null, source: 'realtime' },
    's1',
    11_000,
  );
  assert.deepEqual(statusPresentation(interrupted, copy), { text: '已中断', dot: 'stopped' });
  // A live running entry has no distinguishing token to preserve: the plain
  // completion settles it as a real success.
  const running = mergeOverlayEntry(null, ledgerRead({ done: false, status: 'running' }), 's1', 10_000);
  const completed = mergeOverlayEntry(
    running,
    { sessionId: 's1', agentId: 'agent_1', done: true, failed: false, status: null, source: 'realtime' },
    's1',
    11_000,
  );
  assert.equal(statusPresentation(completed, copy).text, '已完成');
  assert.equal(completed.completedAt, 11_000, 'the success window is granted for the genuine flip');
});

test('mergeOverlayEntry: blocked reads cold-start cleanly; unblocking is terminal-to-terminal, not a flip', () => {
  const now = 10_000;
  // A blocked entry is done in the authority chain: a cold-start blocked read
  // grants no success window, and it never rides the recent list (the
  // visibility test above pins it to the active bucket instead).
  const coldBlocked = mergeOverlayEntry(null, ledgerRead({ done: true, blocked: true, status: 'waiting_input' }), 's1', now);
  assert.equal(coldBlocked.completedAt, undefined, 'a blocked entry read cold-start gets no grant either');
  // Clearing the block goes terminal → terminal: no completedAt, so the entry
  // simply leaves the list. A re-awakened worker that completes again grants
  // its window on the running → completed flip instead.
  const unblocked = mergeOverlayEntry(coldBlocked, ledgerRead({ done: true, blocked: false, status: 'completed' }), 's1', now + 500);
  assert.equal(unblocked.completedAt, undefined, 'blocked → completed is terminal-to-terminal, not a live flip');
  const visible = overlayVisibleEntries([unblocked], now + 500);
  assert.deepEqual(visible.active, []);
  assert.deepEqual(visible.recent, [], 'an unblocked entry leaves the list instead of flashing the success window');
});

test('mergeOverlayEntry: entry ownership follows the passed-in session; cross-session events do not leak', () => {
  const merged = mergeOverlayEntry(null, ledgerRead({ sessionId: 'other' }), 's1', 10_000);
  assert.equal(merged.sessionId, 's1', 'when detail lacks or carries a mismatched sessionId, the subscribing session wins');
});

test('component glue: session-switch discard, revival kick, and ledger mapping stay wired', () => {
  // Session switch drops the previous session's cache entries (cache hygiene
  // on top of the ownership stamping pinned above).
  assert.match(
    overlaySource,
    /entry\.sessionId === sessionId\) continue;[\s\S]{0,300}commitEntries\(next\)/,
    'switching sessions must drop the previous session cache entries',
  );
  // The render projection itself is session-filtered (second half of the
  // cross-session guard).
  assert.match(
    overlaySource,
    /Object\.values\(entries\)\.filter\(entry => entry\.sessionId === sessionId\)/,
    'render must filter entries to the subscribing session',
  );
  // A live non-terminal event rejected by the terminal ratchet kicks an
  // immediate authoritative read instead of waiting for the next heartbeat.
  assert.match(
    overlaySource,
    /if \(!applied && !detail\.done && detail\.source !== 'ledger' && kickPollRef\.current\) \{\s*kickPollRef\.current\(\);/,
    'a ratchet-rejected live event must kick the ledger poll',
  );
  // mergeLedgerSummaries is the only snake_case → camelCase adapter between
  // the Rust ledger summary and the pure model; its field set must stay exact.
  for (const mapping of [
    'agentId: summary.agent_id',
    'role: summary.role || null',
    'status: summary.status || null',
    'done: !!summary.done',
    'failed: !!summary.failed',
    'blocked: !!summary.blocked',
  ]) {
    assert.ok(overlaySource.includes(mapping), `ledger mapping drift: missing \`${mapping}\``);
  }
  // Orphan-transcript rows (done=false, no status token; the foundation
  // prunes worker records past 256 but keeps their transcript files) must be
  // skipped before the merge, or they become eternal "working" ghosts.
  assert.ok(
    overlaySource.includes('if (isUnknownLedgerRow(summary)) continue;'),
    'unknown ledger rows must be skipped before merging into the overlay',
  );
  // The poll hook itself is exercised by its own test file, but this wiring —
  // summaries flowing into the merge and revival hints flowing into the kick —
  // lives only here: swapping either for a no-op would leave every assertion
  // above green while the overlay goes blind.
  assert.match(
    overlaySource,
    /useSubagentLedgerPoll\(\{[\s\S]{0,200}hasActive: sessionHasActive,[\s\S]{0,200}onSummaries: mergeLedgerSummaries,[\s\S]{0,200}kickRef: kickPollRef,[\s\S]{0,50}\}\);/,
    'the ledger poll must be wired to the session activity, the merge callback, and the revival kick',
  );
  // The read itself is the overlay's only data source: the hook test injects
  // its own readLedger, so only this pin sees the component wire silence the
  // facade (same blind-overlay failure as above).
  assert.match(
    overlaySource,
    /const readLedger = useCallback\(\s*id => bridge\.multiAgent\.listSubagentTranscripts\(id\),/,
    'readLedger must call the bridge multiAgent ledger facade',
  );
  // And the finished overlay must actually be mounted by the chat view — an
  // unmount would leave every assertion in this file green with the feature
  // gone from the screen.
  const chatViewSource = fs.readFileSync(
    new URL('../src/features/chat/ChatView.jsx', import.meta.url),
    'utf8',
  );
  assert.match(
    chatViewSource,
    /<RunningAgentsOverlay\s+sessionId=\{activeSessionId\}/,
    'the running overlay must stay mounted in ChatView',
  );
  // Scheduled run conversations assemble plain engine config (the backend's
  // swarm_mode_available excludes them), so the toggle entry must hide there
  // instead of erroring on click.
  assert.match(
    chatViewSource,
    /multiAgentAvailable=\{!scheduledRunContext\}/,
    'scheduled run conversations must hide the swarm toggle entry',
  );
});
