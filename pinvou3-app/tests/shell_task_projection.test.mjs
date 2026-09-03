import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const terminalPath = path.join(appRoot, 'src', 'platform', 'tauri', 'bridge', 'terminal.js');
const terminalSource = fs.readFileSync(terminalPath, 'utf8');

function createTerminal(initialItems = []) {
  const chatItems = structuredClone(initialItems);
  const windowObject = {
    __PINVOU_TAURI_BRIDGE_FEATURES__: {},
    setTimeout,
  };
  const scriptContext = vm.createContext({
    window: windowObject,
    TextEncoder,
    console,
    setTimeout,
    clearTimeout,
  });
  vm.runInContext(terminalSource, scriptContext, { filename: terminalPath });

  let notifications = 0;
  const terminal = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__.terminal({
    state: { chatItems, activeSessionId: 'session-current' },
    notify() { notifications += 1; },
    invoke: async () => [],
    bt(key) {
      if (key === 'shellOutputOmitted') return kind => `${kind} omitted`;
      if (key === 'shellTaskFinished') return code => `finished: ${code}`;
      if (key === 'shellUnknownExit') return 'unknown';
      throw new Error(`unexpected translation key: ${key}`);
    },
    runSyncOnSession(_sessionId, callback) { callback(); },
    addChatItem(item) { chatItems.push(item); },
  });

  return { chatItems, terminal, notifications: () => notifications };
}

function snapshot(overrides = {}) {
  return {
    id: 'shell-old',
    command: 'winget search "BaiduNetdisk"',
    status: 'failed',
    exit_code: 1,
    elapsed_ms: 300,
    stdout_len: 0,
    stderr_len: 16,
    stdout_tail: '',
    stderr_tail: 'old task failed',
    ...overrides,
  };
}

test('completed historical shell jobs are not inserted into the current timeline', () => {
  const harness = createTerminal([{
    type: 'tool',
    toolId: 'current-wait',
    name: 'exec_shell_wait',
    state: 'done',
    output: '13',
  }]);

  const running = harness.terminal.applyShellSnapshots('session-current', [snapshot()]);

  assert.equal(running, false);
  assert.equal(harness.chatItems.length, 1);
  assert.equal(harness.notifications(), 0);
  assert.equal(harness.chatItems.some(item => item.toolId === 'shell-task:shell-old'), false);
});

test('running detached shell jobs still receive a synthetic status card', () => {
  const harness = createTerminal();

  const running = harness.terminal.applyShellSnapshots('session-current', [snapshot({
    id: 'shell-current',
    command: '1..100',
    status: 'running',
    exit_code: null,
    stdout_len: 2,
    stderr_len: 0,
    stdout_tail: '13',
    stderr_tail: '',
  })]);

  assert.equal(running, true);
  assert.equal(harness.notifications(), 1);
  assert.equal(harness.chatItems.length, 1);
  assert.equal(harness.chatItems[0].toolId, 'shell-task:shell-current');
  assert.equal(harness.chatItems[0].taskId, 'shell-current');
  assert.equal(harness.chatItems[0].state, 'running');
  assert.equal(harness.chatItems[0].output, '13');
});

test('completed shell jobs still update their explicitly linked tool card', () => {
  const harness = createTerminal([{
    type: 'tool',
    toolId: 'tool-old',
    taskId: 'shell-old',
    name: 'exec_shell',
    state: 'running',
    args: { command: 'winget search "BaiduNetdisk"' },
    output: null,
  }]);

  harness.terminal.applyShellSnapshots('session-current', [snapshot()]);

  assert.equal(harness.chatItems.length, 1);
  assert.equal(harness.chatItems[0].state, 'failed');
  assert.equal(harness.chatItems[0].success, false);
  assert.match(harness.chatItems[0].output, /old task failed/);
  assert.equal(harness.notifications(), 1);
});

test('a fast detached job can still appear after its start tool', () => {
  const harness = createTerminal([{
    type: 'tool',
    toolId: 'start-fast-detached',
    name: 'exec_shell',
    state: 'done',
    output: 'running in background',
  }]);

  harness.terminal.applyShellSnapshots('session-current', [snapshot({
    id: 'shell-fast-detached',
    command: 'fast-detached',
    status: 'completed',
    exit_code: 0,
    stdout_tail: 'done fast',
    stderr_tail: '',
  })]);

  assert.equal(harness.chatItems.length, 2);
  assert.equal(harness.chatItems[1].toolId, 'shell-task:shell-fast-detached');
  assert.equal(harness.chatItems[1].state, 'done');
  assert.match(harness.chatItems[1].output, /done fast/);
});

test('the web bridge keeps the same stale-completion guard', () => {
  const webBridge = fs.readFileSync(
    path.join(appRoot, 'src', 'platform', 'web', 'bridge.js'),
    'utf8',
  );
  assert.match(
    webBridge,
    /if \(!item && !running && latestShellToolIsWaitObserver\(\)\) return;/,
  );
});
