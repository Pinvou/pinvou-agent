import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

// Behavioral contract for how the "记忆整理" (memory organize) template's kind
// is threaded:
// - kind travels only via createScheduledTask into the create_scheduled_task input;
// - updateScheduledTask / scheduledTaskBackendInput never carry kind
//   (kind is create-only metadata; SCHEDULED_TASK_WRITABLE_FIELDS deliberately omits it).
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = fs.readFileSync(path.join(root, 'src/platform/tauri/bridge/scheduled.js'), 'utf8');
const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
vm.runInNewContext(source, { window: windowObject, setTimeout, clearTimeout, console });

const state = { scheduledTasks: [], scheduledTaskError: null, scheduledTaskBusyAction: null };
const calls = [];
const createdTask = { id: 'task-1', name: '记忆整理', kind: 'memory_organize' };
const invoke = async (command, args) => {
  calls.push([command, args]);
  if (command === 'create_scheduled_task') return createdTask;
  if (command === 'list_scheduled_tasks') return [createdTask];
  return null;
};
const api = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__.scheduled({
  state,
  notify() {},
  invoke,
  bt: key => key,
  runSyncOnSession(_sid, action) { action(); },
  addSystemItem() {},
  rememberScheduledRunOwner() {},
  isScheduledRunTerminal() { return false; },
  createNewSession: async () => {},
  prefillComposer() {},
  sessionStates: {},
});

await api.createScheduledTask({
  name: '记忆整理',
  prompt: '定期整理我的长期记忆',
  rrule: 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=30',
  kind: 'memory_organize',
  selectAfterCreate: false,
});
const createCall = calls.find(([command]) => command === 'create_scheduled_task');
assert.ok(createCall, 'create_scheduled_task must be invoked');
assert.equal(createCall[1].input.kind, 'memory_organize', 'kind must travel with the create input');

calls.length = 0;
await api.updateScheduledTask('task-1', { name: '记忆整理（改名）' });
const updateCall = calls.find(([command]) => command === 'update_scheduled_task');
assert.ok(updateCall, 'update_scheduled_task must be invoked');
assert.equal(updateCall[1].input.kind, undefined, 'update input must never carry kind');

const backendInput = api.scheduledTaskBackendInput({ name: 'n', kind: 'memory_organize' });
assert.equal(backendInput.kind, undefined, 'scheduledTaskBackendInput must strip kind');
assert.deepEqual(
  Object.keys(backendInput).sort(), // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of the string array is the asserted expectation
  ['mode', 'name'],
);

console.log('scheduled bridge kind threading passed');
