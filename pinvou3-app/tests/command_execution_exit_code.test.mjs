import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { projectAuxChatTurns } from '../src/features/aux-chat/aux-chat-state.mjs';

const source = readFileSync(new URL('../src/features/conversation/conversation-model.js', import.meta.url), 'utf8');
const model = await import(new URL('../src/features/conversation/conversation-model.js', import.meta.url));

test('auxiliary Shell failure statistics recover after a successful retry', () => {
  const snapshot = (exitCode, busy = false) => ({
    chatItems: [
      { id: 1, type: 'user', text: 'run command' },
      { id: 2, type: 'tool', name: 'Bash', state: 'done',
        args: { command: 'echo result' },
        output: { output: '\u001b[31mresult\u001b[0m', exit_code: exitCode } },
    ], busy, queued: [],
  });
  const failed = projectAuxChatTurns(snapshot(2), 'aux-shell');
  assert.equal(failed[0].failedOperationCount, 1);
  assert.equal(failed[0].items.find(item => item.type === 'command_execution').tool.rawOutput.exit_code, 2);
  assert.equal(projectAuxChatTurns(snapshot(2, true), 'aux-shell')[0].status, 'running');
  const retried = projectAuxChatTurns(snapshot(0), 'aux-shell');
  assert.equal(retried[0].failedOperationCount, 0);
  assert.equal(retried[0].status, 'completed');
  assert.equal(failed[0].failedOperationCount, 1, 'a retry must not mutate a previous projection');
});

test('exit-code reader follows nullish precedence and numeric conversion', () => {
  assert.equal(typeof model.commandExecutionExitCode, 'function');
  for (const [rawOutput, expected] of [
    [undefined, null], [null, null], ['', null], ['exit_code=1', null],
    [{}, null], [{ exit_code: 0 }, 0], [{ exit_code: '0' }, 0],
    [{ exit_code: '', exitCode: 8 }, null], [{ exit_code: null, exitCode: '8' }, 8],
    [{ exit_code: false }, 0], [{ exit_code: true }, 1], [{ exit_code: ' ' }, 0],
    [{ exit_code: 'invalid' }, null], [{ exit_code: NaN }, null],
    [{ exit_code: -1 }, -1], [{ exit_code: Infinity }, Infinity],
    [{ exitCode: 2n }, 2],
  ]) {
    assert.equal(model.commandExecutionExitCode({ rawOutput }), expected);
  }
});

test('display details keep complete terminal-control cleanup', () => {
  const result = model.commandExecutionDetails({
    rawInput: { command: '\u001b[31mecho hi\u001b[0m\necho bye', cwd: '\u001b[32m/work\u001b[0m' },
    rawOutput: { output: '\u001b[31mfailed\u001b[0m', exit_code: '2' },
  });
  assert.deepEqual(result, {
    command: 'echo hi\necho bye', cwd: '/work', output: 'failed', exitCode: 2,
    summary: 'echo hi', commandCount: 2,
  });
});

test('failure statistics preserve cancellation, interruption and wait exemptions', () => {
  for (const status of ['failed', 'completed', 'done', 'running', 'cancelled', 'canceled', 'Interrupted', 'pending']) {
    for (const code of [null, '', 0, 1, -1, 'bad', Infinity]) {
      const tool = { name: 'Bash', rawInput: { command: 'echo test' }, rawOutput: { exit_code: code, output: 'text' } };
      const expected = model.terminalStatus(status, model.commandExecutionDetails(tool).exitCode) === 'failed';
      assert.equal(model.countsAsFailedOperation({ type: 'command_execution', status, tool }), expected);
      assert.equal(model.countsAsFailedOperation({ type: 'tool', status, tool }), status === 'failed');
    }
    for (const [name, rawInput] of [['agent', { action: 'wait' }], ['agents/wait', {}]]) {
      assert.equal(model.countsAsFailedOperation({ type: 'command_execution', status, tool: { name, rawInput, rawOutput: { exit_code: 1 } } }), false);
    }
  }
});

function observedTool(trace, throwAt = '', failure = new Error('sentinel')) {
  const read = (key, value) => ({ get() { trace.push(key); if (key === throwAt) throw failure; return value; } });
  const rawInput = Object.defineProperties({}, { command: read('command', ''), cwd: read('cwd', '/work') });
  const rawOutput = Object.defineProperties({}, {
    formatted_output: read('formatted_output', ''), output: read('output', null), text: read('text', ''),
    exit_code: read('exit_code', null), exitCode: read('exitCode', '2'),
  });
  return Object.defineProperties({}, {
    rawInput: read('rawInput', rawInput), rawOutput: read('rawOutput', rawOutput),
    content: read('content', ''), title: read('title', 'title'),
  });
}

test('exit-code reading retains display getter order and thrown getter errors', () => {
  const oldTrace = [], nextTrace = [];
  const expected = model.commandExecutionDetails(observedTool(oldTrace)).exitCode;
  assert.equal(model.commandExecutionExitCode(observedTool(nextTrace)), expected);
  assert.deepEqual(nextTrace, oldTrace);
  for (const key of new Set(oldTrace)) {
    const failure = new Error(key), before = [], after = [];
    assert.throws(() => model.commandExecutionDetails(observedTool(before, key, failure)), error => error === failure);
    assert.throws(() => model.commandExecutionExitCode(observedTool(after, key, failure)), error => error === failure);
    assert.deepEqual(after, before);
  }
});

test('exit-code mode retains coercion failures and recovers on the next call', () => {
  for (const field of ['command', 'cwd', 'output', 'exitCode', 'title']) {
    const failure = new Error(field);
    const throwing = { [Symbol.toPrimitive]() { throw failure; } };
    const make = () => ({
      rawInput: { command: field === 'command' ? throwing : '', cwd: field === 'cwd' ? throwing : '' },
      rawOutput: { output: field === 'output' ? throwing : '', exit_code: field === 'exitCode' ? throwing : 0 },
      title: field === 'title' ? throwing : '',
    });
    assert.throws(() => model.commandExecutionDetails(make()), error => error === failure);
    assert.throws(() => model.commandExecutionExitCode(make()), error => error === failure);
    assert.equal(model.commandExecutionExitCode({ rawOutput: { exit_code: 0 } }), 0);
  }
  assert.throws(() => model.commandExecutionExitCode({ rawOutput: { exit_code: Symbol('invalid') } }), TypeError);
});

test('changing content getters retain the final output coercion failure', () => {
  const failure = new Error('changing content coercion');
  const make = () => {
    let reads = 0;
    return { rawOutput: '', get content() {
      reads += 1;
      return reads === 1 ? '' : { [Symbol.toPrimitive]() { throw failure; } };
    } };
  };
  assert.throws(() => model.commandExecutionDetails(make()), error => error === failure);
  assert.throws(() => model.commandExecutionExitCode(make()), error => error === failure);
});

test('failure counting avoids scanning large display output', async () => {
  const sizes = [];
  const instrumented = source.replace('export function stripTerminalControlSequences(value) {',
    'export function stripTerminalControlSequences(value) { globalThis.__exitCodeSizes.push(String(value ?? "").length);')
    .replace("'../../shared/path-utils.js'", () => JSON.stringify(new URL('../src/shared/path-utils.js', import.meta.url).href));
  assert.notEqual(instrumented, source);
  globalThis.__exitCodeSizes = sizes;
  try {
    const measured = await import(`data:text/javascript;base64,${Buffer.from(instrumented).toString('base64')}`);
    const output = '\u001b[31moutput\u001b[0m'.repeat(16384);
    const tool = { name: 'Bash', rawInput: { command: 'echo test' }, rawOutput: { output, exit_code: 2 } };
    assert.equal(measured.countsAsFailedOperation({ type: 'command_execution', status: 'completed', tool }), true);
    assert.ok(sizes.reduce((sum, size) => sum + size, 0) < 1024, 'failure statistics must not normalize the full output');
    assert.equal(measured.commandExecutionDetails(tool).output, 'output'.repeat(16384));
    assert.ok(sizes.includes(output.length), 'display still normalizes the full output');
  } finally {
    delete globalThis.__exitCodeSizes;
  }
});
