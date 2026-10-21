import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import { createNativeToolOutputHarness } from './helpers/native-tool-output-harness.mjs';

const eventSource = fs.readFileSync(new URL('../src/platform/tauri/bridge/chat-events.js', import.meta.url), 'utf8');
const baselineSource = eventSource.replace(/(listen\("chat:tool_delta"[\s\S]*?)}, \{ deferBackgroundNotify: true \}\); \}\);/, '$1}); });');
const item = (host, id = 'tool') => host.snapshot().chatItems.find(value => value.toolId === id);
const delta = (host, content, extra = {}) => host.emit('chat:tool_delta', { session_id: 'background', id: 'tool', content, ...extra });
const start = (host, session_id, id = 'tool', name = 'read_file') => host.emit('chat:tool_start', { session_id, id, name, args: {} });

async function setup(options) {
  const host = createNativeToolOutputHarness(options);
  await host.load('background');
  host.emit('chat:tool_start', { session_id: 'background', id: 'tool', name: 'read_file', args: {} });
  await host.load('foreground');
  host.advance(100);
  return host;
}

test('native background tool bursts publish once at the existing 50ms deadline', async () => {
  const host = await setup();
  const snapshots = [];
  host.api.state.subscribe('chat', snapshot => snapshots.push(snapshot));
  for (let index = 0; index < 100; index++) host.emit('chat:tool_delta', { session_id: 'background', id: 'tool', content: `${index}\n` });
  assert.equal(snapshots.length, 0, 'inactive deltas must not bypass the terminal publication timer');
  host.advance(49);
  assert.equal(snapshots.length, 0);
  host.advance(1);
  assert.equal(snapshots.length, 1);
  await host.load('background');
  assert.equal(host.snapshot().chatItems.find(item => item.toolId === 'tool').output, Array.from({ length: 100 }, (_, index) => `${index}\n`).join(''));
});

test('native foreground tool output retains the existing timer', async () => {
  const host = await setup();
  await host.load('background');
  const snapshots = [];
  host.api.state.subscribe('chat', snapshot => snapshots.push(snapshot));
  host.emit('chat:tool_delta', { session_id: 'background', id: 'tool', content: 'hello' });
  assert.equal(snapshots.length, 0);
  host.advance(50);
  assert.equal(snapshots.length, 1);
  assert.equal(snapshots[0].chatItems.find(item => item.toolId === 'tool').output, 'hello');
});

test('timer-only host coalesces two background sessions into one coherent foreground publication', async () => {
  const host = await setup({ requestAnimationFrame: false });
  await host.load('second');
  start(host, 'second', 'second-tool');
  await host.load('foreground');
  const snapshots = [];
  host.api.state.subscribe('chat', value => snapshots.push(value));
  for (let index = 0; index < 40; index++) {
    delta(host, 'a');
    delta(host, 'b', { session_id: 'second', id: 'second-tool' });
  }
  assert.equal(snapshots.length, 0);
  host.advance(50);
  assert.equal(snapshots.length, 1);
  assert.equal(snapshots[0].chatItems.length, 0);
  await host.load('background');
  assert.equal(item(host).output, 'a'.repeat(40));
  await host.load('second');
  assert.equal(item(host, 'second-tool').output, 'b'.repeat(40));
});

test('A/B/A switches before expiry expose accumulated output without mutating prior subscriptions', async () => {
  const host = await setup();
  await host.load('background');
  const snapshots = [];
  host.api.state.subscribe('chat', value => snapshots.push(value));
  start(host, 'background', 'anchor');
  const old = snapshots.at(-1);
  const frozen = JSON.stringify(old);
  Object.freeze(old.chatItems.find(value => value.toolId === 'tool'));
  await host.load('foreground');
  delta(host, 'pending');
  await host.load('background');
  assert.equal(item(host).output, 'pending');
  assert.equal(JSON.stringify(old), frozen);
  host.advance(50);
  assert.equal(item(host).output, 'pending');
  assert.equal(JSON.stringify(old), frozen);
});

test('split ANSI, carriage returns, mixed streams, Unicode and bounded tails match pre-change routing', async () => {
  const chunks = [
    ['old\r', 'stdout'], ['new\n\u001b[3', 'stdout'], ['1m红色🙂\u001b[0m\n', 'stdout'],
    ['warn\r', 'stderr'], ['警告\n', 'stderr'], ['\u001b]0;title', 'stdout'], ['\u0007done\n', 'stdout'],
  ];
  const values = [];
  for (const source of [baselineSource, eventSource]) {
    const host = await setup({ eventSource: source });
    for (const [content, stream] of chunks) delta(host, content, { stream });
    host.advance(50);
    await host.load('background');
    const parsed = item(host).output;
    assert.ok(parsed.includes('new\n红色🙂\n'));
    assert.ok(parsed.includes('[STDERR] 警告\n'));
    assert.ok(parsed.endsWith('done\n'));
    assert.ok(!parsed.includes('\u001b'));
    await host.load('foreground');
    delta(host, '界'.repeat(150 * 1024));
    host.advance(50);
    await host.load('background');
    assert.equal(item(host).output.length, 128 * 1024 + 2);
    assert.equal(item(host).output, '…\n' + '界'.repeat(128 * 1024));
    values.push({ parsed, tail: item(host).output });
  }
  assert.deepEqual(values[1], values[0]);
});

test('missing, empty and late tool IDs still schedule a bounded trailing publication', async () => {
  const host = await setup();
  const snapshots = [];
  host.api.state.subscribe('chat', value => snapshots.push(value));
  delta(host, 'orphan', { id: undefined });
  delta(host, '', { id: '' });
  delta(host, 'late', { id: 'unknown' });
  assert.equal(snapshots.length, 0);
  host.advance(50);
  assert.equal(snapshots.length, 1);
  await host.load('background');
  assert.equal(item(host).output, null);
  assert.equal(host.snapshot().chatItems.filter(value => value.type === 'tool').length, 1);
});

for (const success of [true, false]) {
  test(`tool_end synchronously publishes authoritative ${success ? 'successful' : 'failed'} output and history`, async () => {
    const host = await setup();
    const snapshots = [];
    host.api.state.subscribe('chat', value => snapshots.push(value));
    delta(host, 'temporary');
    host.emit('chat:tool_end', { session_id: 'background', id: 'tool', output: 'authoritative\u001b[31m', success });
    assert.ok(snapshots.length > 0);
    await host.load('background');
    assert.equal(item(host).output, 'authoritative\u001b[31m');
    assert.equal(item(host).state, success ? 'done' : 'failed');
    const result = host.snapshot().messages.flatMap(message => Array.isArray(message.content) ? message.content : []).find(block => block.type === 'tool_result');
    assert.equal(result.content, 'authoritative\u001b[31m');
    assert.equal(!!result.is_error, !success);
    host.advance(50);
    assert.equal(item(host).output, result.content);
  });
}

test('user input and real tool-gate decisions publish immediately while output is pending', async () => {
  const host = await setup();
  const snapshots = [];
  host.api.state.subscribe('chat', value => snapshots.push(value));
  delta(host, 'pending');
  host.emit('chat:user_input_required', { session_id: 'background', id: 'question', questions: [{ id: 'q', question: 'Continue?', options: [{ label: 'Yes' }] }] });
  assert.ok(snapshots.length > 0);
  const before = snapshots.length;
  host.emit('chat:tool_gate_decision', { session_id: 'background', tool_id: 'gate', tool_name: 'bash', decision: 'denied', reason: 'permission' });
  assert.ok(snapshots.length > before);
  await host.load('background');
  assert.equal(item(host).output, 'pending');
  assert.ok(host.snapshot().chatItems.some(value => value.type === 'user_input' && value.toolCallId === 'question'));
  assert.ok(host.snapshot().chatItems.some(value => value.toolGateDecision && value.decision === 'denied'));
});

test('subscriber exceptions do not wedge later output, including a reentrant delta', async () => {
  const host = await setup();
  const unsubscribe = host.api.state.subscribe('chat', () => { throw new Error('intentional subscriber failure'); });
  delta(host, 'first');
  assert.throws(() => host.advance(50), /intentional subscriber failure/);
  unsubscribe();
  let calls = 0;
  host.api.state.subscribe('chat', () => {
    calls++;
    if (calls === 1) delta(host, 'second');
  });
  delta(host, 'next');
  host.advance(50);
  assert.equal(calls, 1);
  host.advance(50);
  assert.equal(calls, 2);
  await host.load('background');
  assert.equal(item(host).output, 'firstnextsecond');
});

test('deleting a session with a pending timer cannot resurrect its tool buffer', async () => {
  const host = await setup();
  delta(host, 'discard me');
  host.emit('session:deleted', { id: 'background' });
  host.advance(50);
  assert.equal(host.snapshot().chatItems.length, 0);
  await host.load('background');
  assert.equal(item(host), undefined);
});

test('the burst guard detects the original per-chunk native background publications', async () => {
  assert.notEqual(baselineSource, eventSource, 'baseline mutation must remove the candidate routing option');
  const host = await setup({ eventSource: baselineSource });
  let calls = 0;
  host.api.state.subscribe('chat', () => { calls++; });
  for (let index = 0; index < 100; index++) delta(host, 'x');
  assert.equal(calls, 100);
});

test('public cancelGeneration sends native cancellation and Interrupted settles pending output before a fresh retry', async () => {
  const host = await setup();
  await host.load('background');
  host.emit('chat:turn_started', { session_id: 'background' });
  delta(host, 'unfinished\u001b[3');
  await host.api.chat.cancelGeneration();
  assert.ok(host.commands.some(value => value.command === 'cancel_generation' && value.args.sessionId === 'background'));
  host.emit('chat:done', { session_id: 'background', status: 'Interrupted' });
  assert.equal(item(host).state, 'failed');
  assert.equal(item(host).success, false);
  assert.equal(item(host).output, 'unfinished');
  host.advance(50);
  assert.equal(item(host).state, 'failed');
  // A fresh-ID retry in the same session must remain usable.
  host.emit('chat:turn_started', { session_id: 'background' });
  start(host, 'background', 'retry');
  delta(host, 'clean', { id: 'retry' });
  host.emit('chat:tool_end', { session_id: 'background', id: 'retry', output: 'retry complete', success: true });
  assert.equal(item(host, 'retry').output, 'retry complete');
  assert.equal(item(host, 'retry').state, 'done');
  assert.equal(item(host).output, 'unfinished');
});

test('Bash live output and a backgrounded tool_end retain real shell-card semantics', async () => {
  const host = createNativeToolOutputHarness();
  await host.load('background');
  start(host, 'background', 'tool', 'Bash');
  await host.load('foreground');
  host.advance(100);
  let calls = 0;
  host.api.state.subscribe('chat', () => { calls++; });
  delta(host, 'live\n');
  assert.equal(calls, 0);
  host.advance(50);
  assert.equal(calls, 1);
  host.emit('chat:tool_end', { session_id: 'background', id: 'tool', success: true, output: 'detached', metadata: { backgrounded: true, status: 'Running', task_id: 'job' } });
  assert.ok(calls > 1);
  await host.load('background');
  assert.equal(item(host).background, true);
  assert.equal(item(host).state, 'running');
  assert.equal(item(host).output, 'live\n');
});

test('reused provider tool ID and late delta/end after Interrupted preserve baseline final state', async () => {
  const results = [];
  for (const source of [baselineSource, eventSource]) {
    const host = await setup({ eventSource: source });
    delta(host, 'before\u001b[3');
    host.emit('chat:done', { session_id: 'background', status: 'Interrupted' });
    host.emit('chat:turn_started', { session_id: 'background' });
    start(host, 'background');
    delta(host, '1mlate\n');
    host.emit('chat:tool_end', { session_id: 'background', id: 'tool', success: true, output: 'provider final' });
    host.advance(200);
    await host.load('background');
    results.push(JSON.parse(JSON.stringify(host.snapshot())));
  }
  assert.deepEqual(results[1], results[0]);
});
