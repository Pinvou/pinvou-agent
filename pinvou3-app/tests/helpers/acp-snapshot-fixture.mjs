import { appendAcpEvent } from '../../src/features/codex/acp-state.js';

export function snapshotEvent(seq, type = 'agent_message_chunk', data = {}, turnId = 'active', sessionId = 'synthetic') {
  return { version: 1, sessionId, turnId, seq, timestamp: '2026-10-19T00:00:00Z', event: { type, data } };
}

export function snapshotHistory(count) {
  const events = [];
  const add = (id, type, data) => events.push(snapshotEvent(events.length + 1, type, data, id));
  for (let index = 0; index < count; index++) {
    const id = `turn-${index}`;
    const output = `Synthetic ${index}: ${'code output\n'.repeat(128)}`;
    add(id, 'user_message', { content: [{ type: 'text', text: `Task ${index}` }] });
    add(id, 'turn_started', {});
    add(id, 'agent_message_chunk', { content: { type: 'text', text: 'Inspecting' } });
    add(id, 'tool_call', { toolCallId: `tool-${index}`, kind: 'execute', status: 'in_progress', rawInput: { command: 'synthetic' } });
    add(id, 'tool_call_update', { toolCallId: `tool-${index}`, status: 'completed', rawOutput: output });
    add(id, 'agent_message_chunk', { content: { type: 'text', text: `Done ${index}` } });
    add(id, 'turn_completed', { status: 'completed' });
  }
  return events;
}

// Pre-optimization merge contract, intentionally using the unchanged appender.
export function replaySnapshotMerge(snapshot, current, sessionId) {
  return (current || []).filter(event => event?.sessionId === sessionId)
    .reduce((merged, event) => appendAcpEvent(merged, event), snapshot || []);
}
