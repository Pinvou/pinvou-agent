import assert from 'node:assert/strict';
import test from 'node:test';
import { projectAcpTimeline } from '../src/features/codex/acp-state.js';
import { createAcpTimelineProjector } from '../src/features/codex/acp-timeline-projector.js';
import { acpHistory } from './helpers/acp-timeline-fixture.mjs';
const event = (seq, type, data = {}, turnId = 'a', sessionId = 'session') => ({
  seq, turnId, sessionId, timestamp: `2026-10-19T00:00:${String(seq % 60).padStart(2, '0')}Z`,
  event: { type, data },
});
const text = value => ({ content: { type: 'text', text: value } });

test('append preserves completed history references and old published output', () => {
  const project = createAcpTimelineProjector();
  const events = [event(1, 'agent_message_chunk', text('old')), event(2, 'turn_completed'),
    event(3, 'turn_started', {}, 'b')];
  const before = project(events);
  const saved = structuredClone(before);
  const after = project([...events, event(4, 'agent_message_chunk', text('new'), 'b')]);
  assert.deepEqual(after, projectAcpTimeline([...events, event(4, 'agent_message_chunk', text('new'), 'b')]));
  assert.equal(after.turns[0], before.turns[0]);
  assert.equal(after.turns[0].items, before.turns[0].items);
  assert.notEqual(after.turns[1], before.turns[1]);
  assert.deepEqual(before, saved);
});

test('an immutable unchanged log reuses its entire projection', () => {
  const project = createAcpTimelineProjector();
  const events = Object.freeze([event(1, 'turn_started')]);
  assert.equal(project(events), project(events));
});

function freezeTree(value) {
  if (!value || typeof value !== 'object' || Object.isFrozen(value)) return value;
  Object.values(value).forEach(freezeTree);
  return Object.freeze(value);
}

test('every semantic event matches full replay and never mutates published history', () => {
  const project = createAcpTimelineProjector();
  const updates = [
    ['user_message', { content: [{ type: 'text', text: 'question' }], attachments: [{ name: 'sample' }] }],
    ['user_message_chunk', text('more')], ['turn_started', {}],
    ['agent_thought_chunk', text('thinking')], ['agent_message_chunk', text('first')],
    ['agent_message_chunk', { ...text('commentary'), _meta: { codex: { phase: 'commentary' } } }],
    ['tool_call', { toolCallId: 'tool', kind: 'execute', status: 'in_progress', rawInput: { command: 'sample' } }],
    ['permission_requested', { toolCallId: 'tool', request: { options: [{ id: 'allow' }] } }],
    ['permission_resolved', { toolCallId: 'tool', optionId: 'allow', outcome: 'selected' }],
    ['tool_call_update', { update: { toolCallId: 'tool', status: 'completed', rawOutput: { exit_code: 2 } } }],
    ['plan', { entries: [{ content: 'first', status: 'in_progress' }] }],
    ['plan', { entries: [{ content: 'first', status: 'completed' }] }],
    ['elicitation_requested', { elicitationId: 'input', request: { message: 'choose' } }],
    ['elicitation_resolved', { elicitationId: 'input', action: 'accept' }],
    ['usage', { used: 50, size: 100 }], ['agent_message_chunk', text('final')],
    ['turn_completed', { status: 'failed', error: 'synthetic failure' }],
  ];
  let events = freezeTree(acpHistory(3));
  let previous = project(events);
  for (const [type, data] of updates) {
    const saved = structuredClone(previous);
    events = freezeTree([...events, event(events.length + 1, type, data, 'current', 'synthetic')]);
    const next = project(events);
    assert.deepEqual(next, projectAcpTimeline(events), type);
    assert.deepEqual(previous, saved, `${type} mutated prior output`);
    assert.equal(next.turns[0], previous.turns[0], `${type} rebuilt unrelated history`);
    previous = freezeTree(next);
  }
});

test('late updates rebuild their own historical turn and preserve other turns', () => {
  const project = createAcpTimelineProjector();
  const events = acpHistory(5);
  const before = project(events);
  const nextEvents = [...events,
    event(36, 'tool_call_update', { toolCallId: 'tool-0', status: 'failed' }, 'turn-0', 'synthetic'),
    event(37, 'agent_message_chunk', text('late'), 'turn-2', 'synthetic'),
    event(38, 'session_info_update', { title: 'global' }, null, 'synthetic'),
    event(39, 'turn_started', {}, 'new', 'synthetic')];
  const after = project(nextEvents);
  assert.deepEqual(after, projectAcpTimeline(nextEvents));
  assert.notEqual(before.turns[0], after.turns[0]);
  assert.notEqual(before.turns[2], after.turns[2]);
  assert.equal(before.turns[1], after.turns[1]);
  assert.equal(before.turns[4], after.turns[4]);
  assert.equal(after.turns.at(-1).id, 'new');
});

for (const [name, replace] of [
  ['snapshot with new references', history => structuredClone(history)],
  ['truncation', history => history.slice(0, 5)],
  ['out-of-order appended event', history => [...history, event(2, 'agent_message_chunk', text('late'), 'other', 'synthetic')]],
  ['duplicate event', history => [...history, history[0]]],
  ['mixed sessions', history => [...history, event(99, 'turn_started', {}, 'other', 'second')]],
  ['changed event inside prefix', history => [event(1, 'agent_message_chunk', text('replacement'), 'turn-0', 'synthetic'), ...history.slice(1)]],
  ['string sequence', history => [...history, { ...event(99, 'turn_started'), seq: '99' }]],
  ['null envelope', history => [...history, null]],
]) {
  test(`${name} retains full projection behavior and subsequent updates`, () => {
    const project = createAcpTimelineProjector();
    const history = acpHistory(3);
    project(history);
    const replacement = replace(history);
    assert.deepEqual(project(replacement), projectAcpTimeline(replacement));
    const next = [...replacement, event(100, 'agent_message_chunk', text('next'), 'next', 'synthetic')];
    assert.deepEqual(project(next), projectAcpTimeline(next));
  });
}

test('sequence gaps and global-only appends do not discard events', () => {
  const project = createAcpTimelineProjector();
  const initial = [event(1, 'session_info_update', {}, null)];
  project(initial);
  const events = [...initial, event(20, 'turn_started'), event(30, 'session_info_update', {}, null)];
  assert.deepEqual(project(events), projectAcpTimeline(events));
});

test('non-finite or missing appended sequence values retain full replay ordering', () => {
  for (const seq of [NaN, undefined, Infinity]) {
    const project = createAcpTimelineProjector();
    const initial = [event(1, 'agent_message_chunk', text('first'))];
    project(initial);
    const events = [...initial, event(seq, 'agent_message_chunk', text('second'), 'b')];
    assert.deepEqual(project(events), projectAcpTimeline(events));
    const next = [...events, event(3, 'agent_message_chunk', text('third'), 'c')];
    assert.deepEqual(project(next), projectAcpTimeline(next));
  }
});

test('a truthy non-array input does not poison a subsequent array projection', () => {
  const project = createAcpTimelineProjector();
  const invalid = 'x'; // iterable for full replay, but lacks the array cache's every()
  assert.deepEqual(project(invalid), projectAcpTimeline(invalid));
  const events = [event(1, 'agent_message_chunk', text('first'))];
  assert.deepEqual(project(events), projectAcpTimeline(events));
  const next = [...events, event(2, 'turn_started', {}, 'b')];
  assert.deepEqual(project(next), projectAcpTimeline(next));
});

test('prototype-named turn IDs retain the authoritative global-event fallback', () => {
  for (const id of ['constructor', '__proto__', 'toString', 'hasOwnProperty']) {
    const project = createAcpTimelineProjector();
    const initial = [event(1, 'turn_started')];
    project(initial);
    const events = [...initial, event(2, 'agent_message_chunk', text('sample'), id)];
    assert.deepEqual(project(events), projectAcpTimeline(events), id);
    assert.deepEqual(project([...events, event(3, 'turn_started', {}, 'b')]),
      projectAcpTimeline([...events, event(3, 'turn_started', {}, 'b')]));
  }
});

test('session switches, abandoned render branches and separate owners stay isolated', () => {
  const project = createAcpTimelineProjector();
  const history = acpHistory(2);
  const branch = [...history, event(15, 'agent_message_chunk', text('branch'), 'active', 'synthetic')];
  project(history);
  project(branch);
  assert.deepEqual(project(history), projectAcpTimeline(history));
  const session = [event(1, 'turn_started', {}, 'b', 'another')];
  assert.deepEqual(project(session), projectAcpTimeline(session));
  assert.deepEqual(project(branch), projectAcpTimeline(branch));
  const independent = createAcpTimelineProjector();
  assert.notEqual(independent(history).turns[0], project(history).turns[0]);
  assert.deepEqual(project([]), { turns: [], global: [] });
  assert.deepEqual(project(null), { turns: [], global: [] });
});

test('language and redactor changes refresh history; rendering errors remain retryable', () => {
  const originalHelper = globalThis.PinvouModelServiceErrors;
  const project = createAcpTimelineProjector();
  const events = [event(1, 'turn_completed', { error: 'synthetic error' })];
  try {
    globalThis.PinvouModelServiceErrors = { redactTechnicalDetail: (error, language) => `${language}:${error}` };
    assert.equal(project(events, { language: 'en' }).turns[0].error, 'en:synthetic error');
    assert.equal(project(events, { language: 'ja' }).turns[0].error, 'ja:synthetic error');
    globalThis.PinvouModelServiceErrors.redactTechnicalDetail = () => 'redacted';
    assert.equal(project(events, { language: 'ja' }).turns[0].error, 'redacted');
    const next = [...events, event(2, 'turn_completed', { error: 'next' }, 'b')];
    let fail = true;
    globalThis.PinvouModelServiceErrors = { redactTechnicalDetail: value => {
      if (fail && value === 'next') throw new Error('synthetic renderer failure');
      return value;
    } };
    const before = project(events);
    assert.throws(() => project(next), /synthetic renderer failure/);
    assert.equal(project(events), before);
    fail = false;
    assert.deepEqual(project(next), projectAcpTimeline(next));
  } finally {
    if (originalHelper === undefined) delete globalThis.PinvouModelServiceErrors;
    else globalThis.PinvouModelServiceErrors = originalHelper;
  }
});

test('deterministic interleaved turn updates equal full replay after every append', () => {
  const project = createAcpTimelineProjector();
  let seed = 42;
  const nextRandom = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed; };
  let events = [];
  const types = ['agent_message_chunk', 'agent_thought_chunk', 'tool_call_update', 'turn_completed', 'turn_started'];
  for (let seq = 1; seq <= 300; seq++) {
    const type = types[nextRandom() % types.length];
    const id = `turn-${nextRandom() % 9}`;
    const data = type === 'tool_call_update'
      ? { toolCallId: `tool-${seq % 4}`, status: seq % 2 ? 'in_progress' : 'completed' }
      : type === 'turn_completed' ? { status: seq % 2 ? 'interrupted' : 'completed' } : text(`chunk-${seq}`);
    events = [...events, event(seq, type, data, id)];
    assert.deepEqual(project(events), projectAcpTimeline(events), `event ${seq}`);
  }
});
