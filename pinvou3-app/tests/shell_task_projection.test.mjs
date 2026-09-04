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

test('the guard is not disarmed by a running job projected in the same poll', () => {
  const harness = createTerminal([{
    type: 'tool',
    toolId: 'current-wait',
    name: 'exec_shell_wait',
    state: 'done',
    output: '13',
  }]);

  const running = harness.terminal.applyShellSnapshots('session-current', [
    snapshot({
      id: 'shell-live',
      command: 'long task',
      status: 'running',
      exit_code: null,
      stdout_len: 0,
      stderr_len: 0,
      stdout_tail: '',
      stderr_tail: '',
    }),
    snapshot(),
  ]);

  assert.equal(running, true);
  assert.equal(harness.chatItems.length, 2);
  assert.equal(harness.chatItems[1].toolId, 'shell-task:shell-live');
  assert.equal(harness.chatItems[1].state, 'running');
  assert.equal(harness.chatItems.some(item => item.toolId === 'shell-task:shell-old'), false);
  assert.equal(harness.notifications(), 1);
});

test('the canonical Bash action=wait card also arms the guard', () => {
  // Since engine v0.9.3 the wait observer is Bash with action="wait"
  // (exec_shell_wait/exec_wait survive only in replayed legacy sessions).
  const harness = createTerminal([{
    type: 'tool',
    toolId: 'bash-wait',
    name: 'Bash',
    state: 'done',
    args: { action: 'wait', task_id: 'task-1' },
    output: '13',
  }]);

  const running = harness.terminal.applyShellSnapshots('session-current', [snapshot()]);

  assert.equal(running, false);
  assert.equal(harness.chatItems.length, 1);
  assert.equal(harness.notifications(), 0);
  assert.equal(harness.chatItems.some(item => item.toolId === 'shell-task:shell-old'), false);
});

test('a Bash action=run card is a start tool and does not arm the guard', () => {
  // Accepted limit: the guard stays off behind a start tool, so a very short
  // detached job whose first snapshot is terminal still gets its card.
  const harness = createTerminal([{
    type: 'tool',
    toolId: 'bash-run',
    name: 'Bash',
    state: 'done',
    args: { action: 'run', command: 'fast-detached' },
    output: 'running in background',
  }]);

  harness.terminal.applyShellSnapshots('session-current', [snapshot({
    id: 'shell-fast-after-run',
    command: 'fast-detached',
    status: 'completed',
    exit_code: 0,
    stdout_tail: 'done fast',
    stderr_tail: '',
  })]);

  assert.equal(harness.chatItems.length, 2);
  assert.equal(harness.chatItems[1].toolId, 'shell-task:shell-fast-after-run');
});

test('a brand-new subagent job first seen terminal after an older wait card is conservatively hidden', () => {
  // Accepted limit until origin identity lands: the timeline alone cannot
  // tell this job apart from retained older work, so it stays hidden.
  const harness = createTerminal([{
    type: 'tool',
    toolId: 'old-wait',
    name: 'exec_shell_wait',
    state: 'done',
    output: 'ok',
  }]);

  const running = harness.terminal.applyShellSnapshots('session-current', [snapshot({
    id: 'shell-fast-subagent',
    command: 'echo hi',
    status: 'completed',
    exit_code: 0,
    stdout_tail: 'hi',
    stderr_tail: '',
  })]);

  assert.equal(running, false);
  assert.equal(harness.chatItems.length, 1);
  assert.equal(harness.chatItems[0].toolId, 'old-wait');
  assert.equal(harness.notifications(), 0);
});

test('runtime origin identity reconciles a completed job at its original tool card', () => {
  const harness = createTerminal([{
    type: 'tool',
    toolId: 'tool-old',
    name: 'exec_shell',
    state: 'done',
    args: { command: 'different rendered command' },
    output: 'starting',
  }, {
    type: 'tool',
    toolId: 'current-wait',
    name: 'exec_shell_wait',
    state: 'done',
    output: '13',
  }]);

  harness.terminal.applyShellSnapshots('session-current', [snapshot({
    origin_tool_call_id: 'tool-old',
    origin_turn_id: 'turn-old',
  })]);

  assert.equal(harness.chatItems.length, 2);
  assert.equal(harness.chatItems[0].taskId, 'shell-old');
  assert.equal(harness.chatItems[0].originToolCallId, 'tool-old');
  assert.equal(harness.chatItems[0].originTurnId, 'turn-old');
  assert.match(harness.chatItems[0].output, /old task failed/);
  assert.equal(harness.chatItems[1].output, '13');
});

test('identified running root jobs stay visible when their origin card is not loaded', () => {
  const harness = createTerminal();

  const running = harness.terminal.applyShellSnapshots('session-current', [snapshot({
    status: 'running',
    exit_code: null,
    origin_tool_call_id: 'tool-before-compaction',
    origin_turn_id: 'turn-old',
  })]);

  assert.equal(running, true);
  assert.equal(harness.chatItems.length, 1);
  assert.equal(harness.chatItems[0].toolId, 'shell-task:shell-old');
  assert.equal(harness.chatItems[0].state, 'running');
  assert.equal(harness.notifications(), 1);
});

test('identified completed root jobs without their origin card stay out of the current tail', () => {
  const harness = createTerminal();

  const running = harness.terminal.applyShellSnapshots('session-current', [snapshot({
    origin_tool_call_id: 'tool-before-compaction',
    origin_turn_id: 'turn-old',
  })]);

  assert.equal(running, false);
  assert.equal(harness.chatItems.length, 0);
  assert.equal(harness.notifications(), 0);
});

test('the web bridge keeps the same stale-completion guard', () => {
  const webBridge = fs.readFileSync(
    path.join(appRoot, 'src', 'platform', 'web', 'bridge.js'),
    'utf8',
  );
  assert.match(
    webBridge,
    /if \(!item && !running && suppressUnmatchedTerminal\) return;/,
  );
  // The web helper scans a different name set (its SHELL_TOOL_NAMES lacks the
  // wait names), so the union clause is the real cross-bridge parity point.
  assert.match(
    webBridge,
    /\(isShellExecutionTool\(item\.name\) \|\| SHELL_WAIT_TOOL_NAMES\.includes\(item\.name\)\)/,
  );
  assert.match(
    webBridge,
    /const suppressUnmatchedTerminal = latestShellToolIsWaitObserver\(\);/,
  );
  // The canonical Bash action=wait recognition must exist on both bridges.
  assert.match(
    webBridge,
    /item\.name === "Bash" && item\.args != null && item\.args\.action === "wait"/,
  );
  assert.match(
    webBridge,
    /if \(!item && !running && job\.origin_tool_call_id && !job\.owner_agent_id\) return;/,
  );
});
