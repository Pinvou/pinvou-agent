export function acpEvent(seq, type, data = {}, turnId = 'active', sessionId = 'synthetic') {
  return { version: 1, sessionId, turnId, seq,
    timestamp: new Date(Date.UTC(2026, 9, 19, 0, 0, seq)).toISOString(), event: { type, data } };
}

export function acpHistory(count) {
  const events = [];
  const add = (id, type, data) => events.push(acpEvent(events.length + 1, type, data, id));
  for (let index = 0; index < count; index++) {
    const id = `turn-${index}`;
    const output = `Synthetic ${index}: ${'code output\n'.repeat(128)}`;
    add(id, 'user_message', { content: [{ type: 'text', text: `Task ${index}` }] });
    add(id, 'turn_started', {});
    add(id, 'agent_message_chunk', { content: { type: 'text', text: 'Inspecting' } });
    add(id, 'tool_call', { toolCallId: `tool-${index}`, kind: 'execute', status: 'in_progress',
      title: 'Bash', rawInput: { command: 'synthetic' }, content: [] });
    add(id, 'tool_call_update', { toolCallId: `tool-${index}`, status: 'completed', rawOutput: output,
      content: [{ type: 'content', content: { type: 'text', text: output } }] });
    add(id, 'agent_message_chunk', { content: { type: 'text', text: `Done ${index}` } });
    add(id, 'turn_completed', { status: 'completed' });
  }
  return events;
}
