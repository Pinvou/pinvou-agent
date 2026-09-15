#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => readFileSync(path.join(root, ...parts), 'utf8');

// Execute the modular Tauri bridge and prove both v0.9.12 foundation events
// reach a user-visible system item, rather than stopping at an unconsumed emit.
const tauriSource = read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
vm.runInContext(tauriSource, vm.createContext({
  window: windowObject,
  console,
  Date,
  String,
}), { filename: 'chat-events.js' });

const listeners = new Map();
const state = {
  activeSessionId: 'session-1',
  chatItems: [],
  messages: [],
  tokens: { input: 0, max: 0 },
  thinking: { active: false },
};
const copy = {
  compactCancel: 'Context compaction canceled',
  compactAuto: ' (auto)',
  toolGateDecision: 'Permission gate',
  toolGateAllowed: 'allowed',
  toolGateDenied: 'denied',
  toolGateUnavailable: 'could not review and denied',
  toolGateAgent: 'agent',
  toolGateRisk: 'risk',
};
const context = {
  state,
  listen(name, handler) { listeners.set(name, handler); },
  notify() {},
  invoke: async () => null,
  turnUsageDirty: {},
  sessionStates: {},
  renderMarkdown(text) { return text; },
  bt(key) { return copy[key] || key; },
  onSessionEvent(_event, callback) { callback(); },
  runSyncOnSession(_sessionId, callback) { callback(); },
  addChatItem(item) { state.chatItems.push(item); },
  addSystemItem(text, meta = {}) {
    context.addChatItem({ type: 'system', text, ...meta });
  },
  timeStr() { return '12:00'; },
  flushPendingTextBlock() {},
};

windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'](context);
assert.ok(listeners.has('chat:compaction'));
assert.ok(listeners.has('chat:tool_gate_decision'));

const emit = (name, payload) => listeners.get(name)({
  event: name,
  payload: { session_id: 'session-1', ...payload },
});

emit('chat:compaction', { id: 'compact-1', phase: 'cancel', auto: true, message: 'interrupted' });
const cancelNotice = state.chatItems.at(-1);
assert.equal(cancelNotice.type, 'system');
assert.equal(cancelNotice.compactPhase, 'cancel');
assert.match(cancelNotice.text, /Context compaction canceled/);
assert.match(cancelNotice.text, /interrupted/);

emit('chat:tool_gate_decision', {
  tool_id: 'tool-1',
  tool_name: 'bash',
  agent_id: 'reviewer',
  decision: 'denied',
  risk: 'high',
  reason: 'outside delegated scope',
});
const gateNotice = state.chatItems.at(-1);
assert.equal(gateNotice.type, 'system');
assert.equal(gateNotice.toolGateDecision, true);
assert.equal(gateNotice.toolId, 'tool-1');
assert.equal(gateNotice.toolName, 'bash');
assert.equal(gateNotice.decision, 'denied');
assert.equal(gateNotice.reason, 'outside delegated scope');
assert.equal(gateNotice.risk, 'high');
assert.match(gateNotice.text, /Permission gate: bash — denied/);
assert.match(gateNotice.text, /outside delegated scope/);

// The web bridge is a monolithic transport counterpart. Lock the same two
// consumer branches into its registered event section so future regeneration
// cannot silently retain only the Tauri projection.
const webSource = read('src', 'platform', 'web', 'bridge.js');
const webSection = webSource.slice(
  webSource.indexOf('listen("chat:compaction"'),
  webSource.indexOf('listen("chat:user_input_required"'),
);
assert.ok(webSection.includes('phase === "cancel"'));
assert.ok(webSection.includes('bt("compactCancel")'));
assert.ok(webSection.includes('listen("chat:tool_gate_decision"'));
assert.ok(webSection.includes('toolGateDecision: true'));
assert.ok(webSection.includes('toolName: String(p.tool_name || "")'));
assert.ok(webSection.includes('reason: String(p.reason || "")'));
assert.ok(webSection.includes('risk: String(p.risk || "")'));
assert.match(
  read('src', 'features', 'codex', 'CodexAcpView.jsx'),
  /!hasStructuredAuditDetails && legacy\.text/,
  'legacy persisted notices must retain their already-rendered audit text',
);

const forwarderSource = read('src-tauri', 'src', 'features', 'assistant', 'forwarder.rs');
const compactionFailedSection = forwarderSource.slice(
  forwarderSource.indexOf('Event::CompactionFailed'),
  forwarderSource.indexOf('Event::Error { envelope', forwarderSource.indexOf('Event::CompactionFailed')),
);
assert.match(compactionFailedSection, /Event::CompactionFailed \{ id, message, auto \}/);
assert.match(compactionFailedSection, /"id": id/);

console.log('foundation_event_projection.test.mjs: OK');
