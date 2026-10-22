import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { createNativeToolOutputHarness } from './helpers/native-tool-output-harness.mjs';

const sharedSource = fs.readFileSync(new URL('../src/shared/bridge-shared-helpers.js', import.meta.url), 'utf8');
const terminalSource = fs.readFileSync(new URL('../src/platform/tauri/bridge/terminal.js', import.meta.url), 'utf8');

function harness({ web = false } = {}) {
  let now = 0;
  let serial = 0;
  const timers = new Map();
  const warnings = [];
  const publications = [];
  const buffers = new Map([['background', []], ['other', []], ['foreground', []]]);
  const state = { activeSessionId: 'foreground', chatItems: buffers.get('foreground') };
  const setTimeout = (callback, delay) => {
    const id = ++serial;
    timers.set(id, { at: now + delay, callback });
    return id;
  };
  const window = { setTimeout, clearTimeout: id => timers.delete(id) };
  const sandbox = vm.createContext({ window, setTimeout, clearTimeout: window.clearTimeout, TextEncoder,
    console: { warn: (...args) => warnings.push(args) } });
  vm.runInContext(sharedSource, sandbox);
  vm.runInContext(terminalSource, sandbox);
  let subscriber = () => {};
  let jobs = [];
  let ipcError;
  let formatterError;
  const invokeTimes = [];
  const deps = {
    state,
    notify() {
      publications.push({ at: now, buffers: structuredClone([...buffers]) });
      subscriber();
    },
    async invoke() {
      invokeTimes.push(now);
      if (ipcError) throw ipcError;
      return jobs;
    },
    bt(key) {
      if (formatterError) throw formatterError;
      if (key === 'shellTaskFinished') return code => `finished ${code}`;
      if (key === 'shellOutputOmitted') return kind => `${kind} omitted`;
      return 'unknown';
    },
    runSyncOnSession(sid, callback) {
      if (!buffers.has(sid)) return;
      const foreground = state.chatItems;
      state.chatItems = buffers.get(sid);
      try { callback(); } finally { state.chatItems = foreground; }
    },
    addChatItem(item) { state.chatItems.push(item); },
  };
  let sharedDeps;
  const originalCreate = window.PinvouBridgeShared.create;
  window.PinvouBridgeShared = { ...window.PinvouBridgeShared, create: (cluster, injected) => {
    sharedDeps = injected;
    return originalCreate(cluster, injected);
  } };
  const terminal = web ? originalCreate('tauriTerminal', { ...deps,
    SHELL_TOOL_NAMES: ['bash'], shellPollState: Object.create(null),
    normalizeTerminalTail: text => text, latestShellToolIsWaitObserver: () => false,
  }) : window.__PINVOU_TAURI_BRIDGE_FEATURES__.terminal(deps);
  return {
    terminal, state, buffers, publications, warnings, invokeTimes, timers,
    poll: () => sharedDeps.shellPollState.background,
    subscriber: fn => { subscriber = fn; },
    jobs: value => { jobs = value; },
    ipcError: value => { ipcError = value; },
    formatterError: value => { formatterError = value; },
    advance(ms) {
      const until = now + ms;
      for (;;) {
        const next = [...timers].filter(([, timer]) => timer.at <= until)
          .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
        if (!next) break;
        timers.delete(next[0]);
        now = next[1].at;
        next[1].callback();
      }
      now = until;
    },
  };
}

const job = (extra = {}) => ({ id: 'job', status: 'Running', command: 'synthetic',
  origin_tool_call_id: 'tool', origin_turn_id: 'turn', owner_agent_id: null,
  stdout_tail: 'old', stdout_len: 3, stderr_tail: '', stderr_len: 0,
  exit_code: null, elapsed_ms: 1, ...extra });
const prime = host => host.terminal.applyShellSnapshots('background', [job()]);
const progress = host => host.terminal.applyShellSnapshots('background', [job({ stdout_tail: 'new', elapsed_ms: 2 })]);
const settle = () => new Promise(resolve => { setImmediate(resolve); });

test('successful inactive ordinary poll publication consumes its pending shell timer synchronously', () => {
  const host = harness();
  prime(host);
  host.terminal.scheduleShellNotify();
  host.advance(30);
  progress(host);
  assert.equal(host.publications.length, 2);
  host.advance(20);
  assert.equal(host.publications.length, 2, 'the successful poll already published pending shell output');
  assert.equal(host.buffers.get('background')[0].output, 'new');
  host.terminal.scheduleShellNotify();
  host.advance(50);
  assert.equal(host.publications.length, 3, 'consuming one timer must not wedge later output');
});

test('new, binding, status, terminal and mixed terminal batches retain pending publication', () => {
  const variants = [
    h => { h.buffers.get('background').length = 0; },
    h => { delete h.buffers.get('background')[0].taskId; },
    h => { h.buffers.get('background')[0].shellStatus = 'running'; },
    h => { h.buffers.get('background')[0].originTurnId = 'different'; },
    h => { h.buffers.get('background')[0].originToolCallId = 'different'; },
    h => { h.buffers.get('background')[0].sessionId = 'different'; },
    h => { h.buffers.get('background')[0].state = 'failed'; },
    h => { h.buffers.get('background')[0].exitCode = 1; },
    h => { h.buffers.get('background')[0].success = true; },
    h => { delete h.buffers.get('background')[0].shellSnapshotKey; },
  ];
  for (const mutate of variants) {
    const host = harness();
    prime(host);
    host.terminal.scheduleShellNotify();
    mutate(host);
    progress(host);
    const count = host.publications.length;
    host.advance(50);
    assert.equal(host.publications.length, count + 1);
  }
  const host = harness();
  prime(host);
  host.terminal.applyShellSnapshots('background', [job({ id: 'sibling' })]);
  host.terminal.scheduleShellNotify();
  // Both cards already exist: only a per-changed-job policy notices completion.
  host.terminal.applyShellSnapshots('background', [job({ stdout_tail: 'new', status: 'Completed', exit_code: 0 }), job({ id: 'sibling', stdout_tail: 'new' })]);
  const count = host.publications.length;
  host.advance(50);
  assert.equal(host.publications.length, count + 1);
  assert.equal(host.buffers.get('background')[0].state, 'done');
  assert.equal(host.buffers.get('background')[1].state, 'running');
});

test('terminal and unknown statuses stay immediate without consuming a pending timer', () => {
  for (const status of ['Completed', 'Failed', 'Cancelled', 'Unknown']) {
    const host = harness();
    prime(host);
    host.terminal.scheduleShellNotify();
    host.terminal.applyShellSnapshots('background', [job({ status, stdout_tail: 'final', exit_code: 0 })]);
    assert.equal(host.publications.length, 2);
    assert.equal(host.buffers.get('background')[0].state, status === 'Completed' ? 'done' : 'failed');
    host.advance(50);
    assert.equal(host.publications.length, 3);
  }
});

test('quiet and absent session polls cannot consume another buffer publication', () => {
  const host = harness();
  prime(host);
  host.terminal.scheduleShellNotify();
  host.terminal.applyShellSnapshots('background', [job({ elapsed_ms: 900 })]);
  host.terminal.applyShellSnapshots('absent', [job({ stdout_tail: 'new' })]);
  assert.equal(host.publications.length, 1);
  assert.equal(host.buffers.has('absent'), false);
  host.advance(50);
  assert.equal(host.publications.length, 2);
});

test('a successful poll consumes its timer without extending inFlight or the 250ms cadence', async () => {
  const host = harness();
  prime(host);
  host.terminal.scheduleShellNotify();
  host.jobs([job({ stdout_tail: 'new' })]);
  host.terminal.scheduleShellPoll('background', true);
  host.advance(0);
  await settle();
  assert.equal(host.poll().inFlight, false);
  assert.equal(host.poll().waitBudget, 12);
  assert.equal(host.publications.length, 2);
  host.advance(50);
  assert.equal(host.publications.length, 2);
  host.advance(200);
  await settle();
  assert.deepEqual(host.invokeTimes, [0, 250]);
  assert.equal(host.poll().waitBudget, 12);
});

test('foreground and default shared publication preserve the pending native timer', () => {
  const host = harness();
  prime(host);
  host.terminal.scheduleShellNotify();
  host.state.activeSessionId = 'background';
  progress(host);
  host.advance(50);
  assert.equal(host.publications.length, 3);
  const web = harness({ web: true });
  prime(web);
  progress(web);
  assert.equal(web.publications.length, 2, 'an uninjected shared/Web host remains synchronous');
  assert.equal(web.timers.size, 0);
});

test('a throwing subscriber keeps the pending timer and original poll catch, budget and cadence', async () => {
  const host = harness();
  prime(host);
  host.terminal.scheduleShellNotify();
  host.jobs([job({ stdout_tail: 'new' })]);
  const error = new Error('subscriber failed');
  host.subscriber(() => { throw error; });
  host.terminal.scheduleShellPoll('background', true);
  host.advance(0);
  await settle();
  assert.equal(host.poll().waitBudget, 11);
  assert.equal(host.poll().inFlight, false);
  assert.equal(host.warnings.length, 1);
  assert.equal(host.warnings[0][1], error);
  host.subscriber(() => {});
  host.advance(50);
  assert.equal(host.publications.length, 3, 'a failed publication cannot consume timer retry');
  host.advance(200);
  await settle();
  assert.deepEqual(host.invokeTimes, [0, 250]);
  assert.equal(host.poll().waitBudget, 11, 'running result leaves budget unchanged after retry');
});

test('reentrant requests retain their first deadline even while an existing timer is pending', () => {
  const host = harness();
  prime(host);
  host.terminal.scheduleShellNotify();
  host.advance(30);
  let first = true;
  host.subscriber(() => {
    if (!first) return;
    first = false;
    host.buffers.get('other').push({ output: 'subscriber mutation' });
    host.terminal.scheduleShellNotify();
  });
  progress(host);
  host.advance(19);
  assert.equal(host.publications.length, 2);
  host.advance(1);
  assert.equal(host.publications.length, 3);
  assert.equal(host.publications.at(-1).at, 50);
  assert.equal(host.publications.at(-1).buffers.find(([sid]) => sid === 'other')[1][0].output, 'subscriber mutation');
});

test('timer clears before callbacks, allowing a fresh timer after reentrant notification', () => {
  const host = harness();
  prime(host);
  host.terminal.scheduleShellNotify();
  let once = true;
  host.subscriber(() => {
    if (!once) return;
    once = false;
    progress(host);
    host.terminal.scheduleShellNotify();
  });
  host.advance(50);
  assert.equal(host.publications.length, 3);
  host.advance(50);
  assert.equal(host.publications.length, 4);
});

test('IPC and formatting failures leave pending timers and the original retry budget', async () => {
  for (const kind of ['ipcError', 'formatterError']) {
    const host = harness();
    prime(host);
    host.terminal.scheduleShellNotify();
    host.jobs([job({ stdout_tail: 'new', stdout_len: 20 })]);
    host[kind](new Error(kind));
    host.terminal.scheduleShellPoll('background', true);
    host.advance(0);
    await settle();
    assert.equal(host.poll().waitBudget, 11);
    assert.equal(host.poll().inFlight, false);
    assert.equal(host.warnings.length, 1);
    host.advance(50);
    assert.equal(host.publications.length, 2);
    host[kind]();
    host.advance(200);
    await settle();
    assert.deepEqual(host.invokeTimes, [0, 250]);
    assert.equal(host.poll().waitBudget, 11);
    assert.equal(host.poll().inFlight, false);
    assert.match(host.buffers.get('background')[0].output, /new$/);
    assert.equal(host.publications.length, 3);
  }
});

test('a replacement shell timer created during publication keeps its own deadline', () => {
  const host = harness();
  prime(host);
  host.terminal.scheduleShellNotify();
  host.advance(30);
  let first = true;
  host.subscriber(() => {
    if (!first) return;
    first = false;
    // Explicit clock reentrancy exercises timer identity replacement; a real
    // native callback ordinarily runs to completion before another timer.
    host.advance(20);
    host.terminal.scheduleShellNotify();
  });
  progress(host);
  assert.equal(host.publications.length, 3);
  host.advance(49);
  assert.equal(host.publications.length, 3);
  host.advance(1);
  assert.equal(host.publications.length, 4);
  assert.equal(host.publications.at(-1).at, 100);
});

test('a pending IPC keeps one inFlight request and retains the original next-poll deadline', async () => {
  const host = harness();
  prime(host);
  let resolve;
  host.jobs(new Promise(done => { resolve = done; }));
  host.terminal.scheduleShellPoll('background', true);
  host.advance(0);
  await settle();
  assert.equal(host.poll().inFlight, true);
  host.terminal.scheduleShellPoll('background', true);
  host.advance(20);
  assert.deepEqual(host.invokeTimes, [0]);
  resolve([job({ stdout_tail: 'new' })]);
  await settle();
  assert.equal(host.poll().inFlight, false);
  host.advance(249);
  assert.deepEqual(host.invokeTimes, [0]);
  host.advance(1);
  await settle();
  assert.deepEqual(host.invokeTimes, [0, 270]);
});

test('repeated poll failures exhaust the unchanged twelve-attempt retry budget', async () => {
  const host = harness();
  prime(host);
  host.ipcError(new Error('offline'));
  host.terminal.scheduleShellPoll('background', true);
  host.advance(0);
  await settle();
  for (let index = 1; index < 12; index++) {
    host.advance(250);
    await settle();
  }
  assert.equal(host.poll(), undefined);
  assert.equal(host.warnings.length, 12);
  assert.equal(host.invokeTimes.length, 12);
  host.advance(500);
  await settle();
  assert.equal(host.invokeTimes.length, 12);
});

async function nativePollHost() {
  let running = true;
  const sid = 'aux-task';
  const host = createNativeToolOutputHarness({ requestAnimationFrame: false,
    shellTasks: (args, now) => args.sessionId === sid ? [job({
      status: running ? 'Running' : 'Cancelled', stdout_tail: `poll ${now}`,
      stdout_len: `poll ${now}`.length,
    })] : [],
  });
  await host.load(sid);
  host.emit('chat:tool_start', { session_id: sid, id: 'tool', name: 'Bash', args: { command: 'synthetic' } });
  await host.load('foreground');
  host.advance(0);
  await settle();
  host.advance(100);
  return { host, sid, stop: () => { running = false; } };
}

async function nativeOverlap(host, sid) {
  // The first poll settled at t=0; the next IPC completes at t=250 while
  // the t=230 delta still has its t=280 publication outstanding.
  host.advance(130);
  host.emit('chat:tool_delta', { session_id: sid, id: 'tool', content: 'delta before poll' });
  host.advance(20);
  await settle();
}

test('actual native poll delivers auxiliary pulls for every pending buffer and preserves LRU residency', async () => {
  const { host, sid } = await nativePollHost();
  await host.load('aux-other');
  host.emit('chat:tool_start', { session_id: 'aux-other', id: 'other', name: 'read_file', args: {} });
  await host.load('foreground');
  const pulls = [];
  host.api.state.subscribe('chat', () => {
    pulls.push([host.api.auxChat.snapshot(sid), host.api.auxChat.snapshot('aux-other')]);
  });
  host.advance(130);
  host.emit('chat:tool_delta', { session_id: sid, id: 'tool', content: 'delta before poll' });
  host.emit('chat:tool_delta', { session_id: 'aux-other', id: 'other', content: 'other pending output' });
  host.advance(20);
  await settle();
  assert.equal(pulls.length, 1, 'the ordinary poll publishes synchronously');
  assert.equal(pulls[0][0].chatItems.find(item => item.toolId === 'tool').output, 'poll 250');
  assert.equal(pulls[0][1].chatItems.find(item => item.toolId === 'other').output, 'other pending output');
  host.advance(30);
  assert.equal(pulls.length, 1, 'the already delivered timer is consumed');
  for (let index = 0; index < 40; index++) await host.load(`switch-${index}`);
  assert.equal(host.api.auxChat.snapshot(sid).chatItems.find(item => item.toolId === 'tool').output, 'poll 250');
  assert.equal(host.api.auxChat.snapshot('aux-other').chatItems.find(item => item.toolId === 'other').output, 'other pending output');
});

test('actual changing-poll overlap survives cancel, interruption and retry without losing authoritative output', async () => {
  const { host, sid, stop } = await nativePollHost();
  await nativeOverlap(host, sid);
  await host.load(sid);
  const old = host.snapshot();
  assert.equal(old.chatItems.find(item => item.toolId === 'tool').output, 'poll 250');
  host.emit('chat:turn_started', { session_id: sid });
  host.emit('chat:tool_delta', { session_id: sid, id: 'tool', content: 'unfinished\u001b[3' });
  await host.api.chat.cancelGeneration();
  assert.ok(host.commands.some(value => value.command === 'cancel_generation' && value.args.sessionId === sid));
  stop();
  host.emit('chat:done', { session_id: sid, status: 'Interrupted' });
  assert.equal(host.snapshot().chatItems.find(item => item.toolId === 'tool').state, 'failed');
  host.emit('chat:turn_started', { session_id: sid });
  host.emit('chat:tool_start', { session_id: sid, id: 'retry', name: 'Bash', args: { command: 'retry' } });
  host.emit('chat:tool_delta', { session_id: sid, id: 'retry', content: 'fresh' });
  host.emit('chat:tool_end', { session_id: sid, id: 'retry', output: 'authoritative retry', success: true });
  host.advance(50);
  await settle();
  const retry = host.snapshot().chatItems.find(item => item.toolId === 'retry');
  assert.equal(retry.state, 'done');
  assert.equal(retry.output, 'authoritative retry');
  assert.equal(old.chatItems.find(item => item.toolId === 'tool').output, 'poll 250', 'prior immutable snapshot remains unchanged');
});

test('actual changing-poll overlap exposes output on activation and cannot resurrect a deleted buffer', async () => {
  const { host, sid } = await nativePollHost();
  await nativeOverlap(host, sid);
  await host.load(sid);
  assert.equal(host.snapshot().chatItems.find(item => item.toolId === 'tool').output, 'poll 250');
  await host.load('foreground');
  await host.load(sid);
  assert.equal(host.snapshot().chatItems.find(item => item.toolId === 'tool').output, 'poll 250');
  await host.load('foreground');
  host.emit('chat:tool_delta', { session_id: sid, id: 'tool', content: 'discard' });
  host.emit('session:deleted', { id: sid });
  host.advance(250);
  await settle();
  assert.equal(host.api.auxChat.snapshot(sid).chatItems.length, 0);
  assert.equal(host.snapshot().chatItems.length, 0);
  await host.load(sid);
  assert.equal(host.snapshot().chatItems.some(item => item.toolId === 'tool'), false);
});
