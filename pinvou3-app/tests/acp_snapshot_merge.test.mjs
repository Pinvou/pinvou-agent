import assert from 'node:assert/strict';
import test from 'node:test';
import { mergeAcpTimelineSnapshot, projectAcpTimeline } from '../src/features/codex/acp-state.js';
import { replaySnapshotMerge, snapshotEvent as event, snapshotHistory } from './helpers/acp-snapshot-fixture.mjs';

function compare(snapshot, current, sessionId = 'synthetic') {
  const expected = replaySnapshotMerge(snapshot, current, sessionId);
  const actual = mergeAcpTimelineSnapshot(snapshot, current, sessionId);
  assert.deepEqual(actual, expected);
  for (let index = 0; index < expected.length; index++) assert.equal(actual[index], expected[index]);
  if (expected === snapshot) assert.equal(actual, snapshot);
  return actual;
}

test('overlap reconciliation reads the snapshot only a linear number of times', () => {
  const history = snapshotHistory(20);
  let reads = 0;
  const snapshot = new Proxy(structuredClone(history), { get(target, key, receiver) {
    if (typeof key === 'string' && /^\d+$/.test(key)) reads++;
    return Reflect.get(target, key, receiver);
  } });
  assert.equal(mergeAcpTimelineSnapshot(snapshot, history, 'synthetic'), snapshot);
  assert.ok(reads <= history.length * 4, `snapshot indexed reads ${reads} exceed linear budget`);
});

test('many novel events require at most one full sort', () => {
  const snapshot = snapshotHistory(2);
  const current = [...snapshot, ...Array.from({ length: 20 }, (_, index) => event(100 - index))];
  const expected = replaySnapshotMerge(snapshot, current, 'synthetic');
  const originalSort = Array.prototype.sort;
  let sorts = 0;
  let result;
  try {
    Array.prototype.sort = function (...args) { sorts++; return Reflect.apply(originalSort, this, args); };
    result = mergeAcpTimelineSnapshot(snapshot, current, 'synthetic');
  } finally {
    Array.prototype.sort = originalSort;
  }
  assert.deepEqual(result, expected);
  assert.ok(sorts <= 1, `full sorts ${sorts} exceed one bulk sort`);
});

test('snapshot wins conflicting duplicate payloads and retains original snapshot duplicates', () => {
  const first = event(1, 'turn_started');
  const repeated = event(1, 'usage', { used: 5 });
  const snapshot = [first, repeated, event(2, 'turn_completed')];
  const actual = compare(snapshot, [event(1, 'usage', { used: 99 }), event(3), event(3)]);
  assert.equal(actual[0], first);
  assert.equal(actual[1], repeated);
  assert.equal(actual.length, 4);
});

test('no-op snapshots preserve identity and order, even for an unsorted snapshot', () => {
  const snapshot = [event(3), event(1), event(2)];
  assert.equal(compare(snapshot, []), snapshot);
  assert.equal(compare(snapshot, [event(2), event(1)]), snapshot);
  assert.deepEqual(snapshot.map(item => item.seq), [3, 1, 2]);
});

test('local events are filtered by session and new events have stable numeric order', () => {
  const snapshot = [event(3), event(1, 'turn_started', {}, 'other', 'another')];
  const current = [event(2), event(1), event(1, 'turn_completed', {}, 'other', 'another'), null];
  const actual = compare(snapshot, current);
  assert.equal(actual.length, 4);
  assert.equal(actual[0], snapshot[1]);
  assert.equal(actual[1], current[1]);
});

test('merge does not mutate frozen input arrays or event payloads', () => {
  const snapshot = Object.freeze(snapshotHistory(2).map(Object.freeze));
  const current = Object.freeze([...snapshot, Object.freeze(event(99))]);
  const savedSnapshot = structuredClone(snapshot);
  const savedCurrent = structuredClone(current);
  compare(snapshot, current);
  assert.deepEqual(snapshot, savedSnapshot);
  assert.deepEqual(current, savedCurrent);
});

test('recovery retains permission, elicitation, tool and terminal events and projections', () => {
  const snapshot = snapshotHistory(2);
  const types = ['turn_started', 'permission_requested', 'permission_resolved', 'elicitation_requested',
    'elicitation_resolved', 'tool_call', 'tool_call_update', 'usage', 'turn_completed'];
  const live = types.map((type, index) => event(snapshot.length + index + 1, type,
    { toolCallId: 'tool', elicitationId: 'question', request: {}, status: 'interrupted' }));
  const current = [...snapshot, event(1, 'turn_started', {}, 'other', 'another'), ...live];
  const actual = compare(snapshot, current);
  assert.deepEqual(actual.slice(snapshot.length), live);
  assert.deepEqual(projectAcpTimeline(actual), projectAcpTimeline(replaySnapshotMerge(snapshot, current, 'synthetic')));
});

test('partial snapshots heal missing events while preserving live tail during fetch', () => {
  const history = snapshotHistory(3);
  const snapshot = structuredClone(history.filter(item => item.seq % 3));
  const current = [...history, event(99, 'permission_requested', { request: {} })];
  const actual = compare(snapshot, current);
  assert.deepEqual(actual.map(item => item.seq), [...history.map(item => item.seq), 99]);
  assert.equal(compare(actual, current), actual);
});

for (const [name, incoming] of [
  ['numeric string', { ...event(1), seq: '1' }],
  ['non-numeric string', { ...event(1), seq: 'invalid' }],
  ['NaN', { ...event(1), seq: Number.NaN }],
  ['Infinity', { ...event(1), seq: Number.POSITIVE_INFINITY }],
  ['negative', { ...event(1), seq: -2 }],
  ['fractional', { ...event(1), seq: 1.5 }],
  ['missing sequence', { sessionId: 'synthetic', event: { type: 'usage' } }],
]) {
  test(`irregular ${name} sequence retains legacy merge behavior`, () => {
    compare([event(2), event(1)], [incoming, event(3), event(4)]);
  });
}

test('legacy exception and missing-input behavior remains intact', () => {
  compare(null, [event(2), event(1)]);
  compare();
  assert.deepEqual(mergeAcpTimelineSnapshot([], [null]), replaySnapshotMerge([], [null]));
  assert.throws(() => mergeAcpTimelineSnapshot([null], [event(1)], 'synthetic'), TypeError);
  assert.throws(() => mergeAcpTimelineSnapshot({}, [event(1)], 'synthetic'), TypeError);
  assert.throws(() => mergeAcpTimelineSnapshot([], {}, 'synthetic'), TypeError);
  const hole = new Array(2);
  hole[1] = event(1);
  compare(hole, [event(2)]);
});

test('accessor-backed sequence values retain the legacy replay and read behavior', () => {
  function run(merge) {
    let reads = 0;
    const item = { ...event(1), get seq() { reads++; return 1; } };
    const value = merge([event(2)], [item, event(3), event(4)], 'synthetic');
    return { value, reads };
  }
  assert.deepEqual(run(mergeAcpTimelineSnapshot), run(replaySnapshotMerge));
});

test('inherited keys and unsafe integer sequences retain legacy replay', () => {
  const inherited = Object.assign(Object.create({ sessionId: 'synthetic', seq: 1 }), { event: { type: 'usage' } });
  compare([event(2)], [inherited, event(Number.MAX_SAFE_INTEGER + 1), event(4)]);
});

test('failed descriptor inspection falls back without changing legacy results', () => {
  const envelope = new Proxy(event(1), { getOwnPropertyDescriptor() { throw new Error('descriptor unavailable'); } });
  compare([envelope], [event(2)]);
});

test('deterministic mixed subsets equal legacy values, references and projections', () => {
  let seed = 93;
  const random = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed; };
  const history = snapshotHistory(4);
  for (let round = 0; round < 300; round++) {
    const snapshot = history.filter(() => random() % 3).map(item => structuredClone(item));
    const current = history.filter(() => random() % 3);
    if (round % 2) snapshot.reverse();
    if (round % 3) current.reverse();
    const actual = compare(snapshot, [...current, ...current.slice(0, 3)]);
    assert.deepEqual(projectAcpTimeline(actual), projectAcpTimeline(replaySnapshotMerge(snapshot, current, 'synthetic')));
  }
});
