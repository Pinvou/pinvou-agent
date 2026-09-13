/** Swarm-mode spawn aggregation (spawn-aggregation.mjs): consecutive spawns → one count row. */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  annotateAgentSpawnGroups,
  isAgentSpawnChatItem,
  spawnGroupOf,
} from '../src/features/multiagent/spawn-aggregation.mjs';

const spawnItem = (id, extra = {}) => ({
  type: 'tool',
  id,
  name: 'agent',
  args: { action: 'start', prompt: `task ${id}` },
  state: 'done',
  success: true,
  output: JSON.stringify({ agent_id: `agent_${id}` }),
  ...extra,
});

test('spawn predicate: only agent calls with a start action and a task body are spawns', () => {
  assert.equal(isAgentSpawnChatItem(spawnItem('aaaa0001')), true);
  assert.equal(isAgentSpawnChatItem(spawnItem('aaaa0002', { args: { action: 'status', agent_id: 'agent_aaaa0001' } })), false);
  assert.equal(isAgentSpawnChatItem(spawnItem('aaaa0003', { args: { action: 'wait', agent_id: 'agent_aaaa0001' } })), false);
  assert.equal(isAgentSpawnChatItem(spawnItem('aaaa0004', { args: { action: 'cancel', agent_id: 'agent_aaaa0001' } })), false);
  assert.equal(isAgentSpawnChatItem({ type: 'tool', name: 'exec_shell', args: { command: 'ls' } }), false);
  assert.equal(isAgentSpawnChatItem(null), false);
});

test('consecutive spawns aggregate into one count row: first carries spawnGroup, rest hidden', () => {
  const items = [
    spawnItem('aaaa0001'),
    spawnItem('aaaa0002'),
    spawnItem('aaaa0003'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated.length, 3);
  assert.equal(annotated[0].spawnGroup.count, 3);
  assert.equal(annotated[0].spawnGroup.failed, 0);
  assert.equal(annotated[1].spawnGroupHidden, true);
  assert.equal(annotated[2].spawnGroupHidden, true);
  assert.ok(!annotated[0].spawnGroupHidden);
});

test('a non-tool content block between spawns breaks the group and starts a new row', () => {
  const items = [
    spawnItem('aaaa0001'),
    { type: 'assistant', text: 'words in between' },
    spawnItem('aaaa0002'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated[0].spawnGroup.count, 1);
  assert.ok(!annotated[1].spawnGroupHidden && !annotated[1].spawnGroup, 'non-tool items are untouched by annotation');
  assert.equal(annotated[2].spawnGroup.count, 1, 'a new sequence counts from 1 again');
});

test('status/wait/cancel coordination calls break the spawn sequence and never count', () => {
  const items = [
    spawnItem('aaaa0001'),
    spawnItem('aaaa0002', { args: { action: 'status', agent_id: 'agent_aaaa0001' } }),
    spawnItem('aaaa0003'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated[0].spawnGroup.count, 1);
  assert.ok(!annotated[1].spawnGroupHidden, 'coordination rows render untouched');
  assert.equal(annotated[2].spawnGroup.count, 1, 'a spawn after a coordination call belongs to a new sequence');
});

test('failed spawns count toward failed without changing the row total', () => {
  const items = [
    spawnItem('aaaa0001', { success: false, output: 'Error: spawn failed' }),
    spawnItem('aaaa0002'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated[0].spawnGroup.count, 2);
  assert.equal(annotated[0].spawnGroup.failed, 1);
});

test('annotateAgentSpawnGroups leaves non-grouped item references untouched', () => {
  const plain = { type: 'user', text: 'hi' };
  const annotated = annotateAgentSpawnGroups([plain]);
  assert.equal(annotated[0], plain);
});

test('items without spawns are returned by reference (new array, same item references)', () => {
  const plain = { type: 'user', text: 'hi' };
  const items = [plain];
  const annotated = annotateAgentSpawnGroups(items);
  assert.notEqual(annotated, items, 'the array is always newly built');
  assert.equal(annotated[0], plain, 'items keep their references');
});

test('degenerate inputs: empty array yields an empty array, non-arrays pass through by reference', () => {
  assert.deepEqual(annotateAgentSpawnGroups([]), []);
  const notAnArray = { length: 1 };
  assert.equal(annotateAgentSpawnGroups(notAnArray), notAnArray, 'non-array input is returned untouched');
  assert.equal(annotateAgentSpawnGroups(null), null);
});

test('the canonical `agents/wait` coordination tool breaks the spawn sequence', () => {
  const items = [
    spawnItem('aaaa0001'),
    { type: 'tool', id: 'w1', name: 'agents/wait', args: {}, state: 'done', success: true },
    spawnItem('aaaa0002'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated[0].spawnGroup.count, 1);
  assert.ok(!annotated[1].spawnGroupHidden && !annotated[1].spawnGroup, 'wait rows render untouched');
  assert.equal(annotated[2].spawnGroup.count, 1, 'a spawn after agents/wait belongs to a new sequence');
});

test('state === "failed" alone (without success === false) counts toward failed', () => {
  const items = [
    spawnItem('aaaa0001', { state: 'failed' }),
    spawnItem('aaaa0002'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated[0].spawnGroup.count, 2);
  assert.equal(annotated[0].spawnGroup.failed, 1);
});

test('annotation never mutates the input items: group members are shallow copies', () => {
  const first = spawnItem('aaaa0001');
  const second = spawnItem('aaaa0002', { success: false });
  const items = [first, second];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated.length, items.length);
  assert.ok(!('spawnGroup' in first), 'the original first spawn gains no annotation fields');
  assert.ok(!('spawnGroupHidden' in second), 'the original hidden spawn gains no annotation fields');
  assert.ok(!('spawnGroup' in items[0]) && !('spawnGroupHidden' in items[1]), 'the input array items stay clean');
});

test('spawnGroupOf: degenerate group shape for a single unannotated spawn item', () => {
  assert.deepEqual(spawnGroupOf(spawnItem('aaaa0001')), { count: 1, failed: 0 });
  assert.deepEqual(spawnGroupOf(spawnItem('aaaa0002', { success: false })), { count: 1, failed: 1 });
  assert.deepEqual(spawnGroupOf(spawnItem('aaaa0003', { state: 'failed' })), { count: 1, failed: 1 });
});
