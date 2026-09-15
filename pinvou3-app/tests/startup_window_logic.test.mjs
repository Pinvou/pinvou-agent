import assert from 'node:assert/strict';
import { revealStartupWindow } from '../src/platform/tauri/startup-window.js';

const calls = [];
const invoke = async (command) => {
  calls.push(command);
  return true;
};

assert.equal(await revealStartupWindow({ search: '?ui=test', invoke }), false);
assert.deepEqual(calls, [], 'non-Linux-dev startup must not invoke the reveal command');

assert.equal(await revealStartupWindow({
  search: '?ui=test&startupWindow=hidden',
  invoke,
}), true);
assert.deepEqual(calls, ['reveal_startup_window']);

assert.equal(await revealStartupWindow({
  search: '?startupWindow=hidden',
  invoke: async () => false,
}), false, 'an already-visible window is a successful no-op');

const warnings = [];
assert.equal(await revealStartupWindow({
  search: '?startupWindow=hidden',
  invoke: async () => { throw new Error('reveal rejected'); },
  warn: (message) => warnings.push(message),
}), false);
assert.equal(warnings.length, 1);

console.log('startup window logic tests passed');
