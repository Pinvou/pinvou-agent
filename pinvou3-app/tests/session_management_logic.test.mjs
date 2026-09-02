import assert from 'node:assert/strict';
import { runSessionBatch, sessionRoute } from '../src/shared/session-management.js';

assert.equal(sessionRoute({ id: 'chat-1' }), 'chat');
assert.equal(sessionRoute({ id: 'sched-1', scheduledRun: { id: 'run-1' } }), 'scheduled');
assert.equal(sessionRoute({ id: 'codex-1', taskKind: 'codex' }), 'codex');

const calls = [];
const archiveResult = await runSessionBatch([
  { id: 'chat-1', taskKind: 'regular' },
  { id: 'codex-1', taskKind: 'codex' },
], 'archive', {
  archive: async id => {
    await Promise.resolve();
    calls.push(`archive:${id}`);
    return true;
  },
  archiveCodex: async id => {
    await Promise.resolve();
    calls.push(`archiveCodex:${id}`);
    return true;
  },
});
assert.deepEqual(calls.sort(), ['archive:chat-1', 'archiveCodex:codex-1']); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
assert.deepEqual(archiveResult, { total: 2, succeeded: 2, failed: 0 });

const partialResult = await runSessionBatch([
  { id: 'chat-ok' },
  { id: 'chat-false' },
  { id: 'chat-rejected' },
], 'delete', {
  delete: async id => {
    if (id === 'chat-false') return false;
    if (id === 'chat-rejected') throw new Error('backend failed');
    return true;
  },
});
assert.deepEqual(partialResult, { total: 3, succeeded: 1, failed: 2 });

const missingHandlerResult = await runSessionBatch([{ id: 'chat-1' }], 'restore', {});
assert.deepEqual(missingHandlerResult, { total: 1, succeeded: 0, failed: 1 });

console.log('session management logic tests passed');
