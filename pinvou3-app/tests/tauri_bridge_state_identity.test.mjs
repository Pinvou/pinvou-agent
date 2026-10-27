#!/usr/bin/env node
/**
 * Desktop bridge state subscription identity contract
 * (subscribeStateSlice / subscribeStateSlices):
 *
 *   - an unrelated publication (the chat domain's composerPrefill) must not
 *     replace the settings/models single-domain slice or the multi-domain
 *     combined snapshot identity, so React can skip unrelated renders via
 *     Object.is;
 *   - callbacks still fire on every publication, so no notification is lost;
 *   - when a subscribed domain really changes, the combined snapshot must get
 *     a new identity carrying the new value, while unchanged domains keep
 *     their slice identity.
 *
 * Mirrors the cross-domain identity assertions in
 * web_bridge_domain_contract.test.mjs (the web stablePick locks the same
 * semantics). Loads platform/tauri/bridge.js in a vm and drives the public
 * methods through a fake __TAURI__; the subscription mechanism itself is not
 * mocked.
 *
 * Run: node --test pinvou3-app/tests/tauri_bridge_state_identity.test.mjs
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');

let invokeResponse = async () => null;
const localStorage = {
  getItem: () => null,
  setItem() {},
  removeItem() {},
};
const windowObject = {
  __TAURI__: {
    core: { invoke: (...args) => invokeResponse(...args) },
    event: { listen: async () => () => {} },
    dialog: { open: async () => null },
  },
  performance: { now: () => 0 },
  addEventListener() {},
  localStorage,
  setTimeout,
  clearTimeout,
};
const context = vm.createContext({
  window: windowObject,
  document: {
    addEventListener() {},
    createElement: () => ({ style: {} }),
    body: { appendChild() {} },
  },
  navigator: {},
  console,
  setTimeout,
  clearTimeout,
  // Feature scripts may register long-lived polling at load time; real timers
  // would pin the event loop and the test does not need polling, so use a
  // stub that never schedules.
  setInterval: () => 0,
  clearInterval() {},
  structuredClone,
  localStorage,
});

// Page load order: shared/bridge-messages.js and shared/bridge-shared-helpers.js
// -> platform/tauri/bridge/*.js factories -> bridge.js (the order itself is
// asserted in classic_startup_bundle.test.mjs; this parses index.html for the
// bridge script order so added or removed feature files are followed
// automatically).
const indexHtml = fs.readFileSync(path.join(root, 'src/index.html'), 'utf8');
const bridgeScripts = [...indexHtml.matchAll(/<script src="%BASE_URL%(shared\/bridge-messages\.js|shared\/bridge-shared-helpers\.js|platform\/tauri\/bridge(?:\.js|\/[^"]+))"[^>]*><\/script>/g)]
  .map(match => match[1]);
assert.ok(bridgeScripts.includes('platform/tauri/bridge.js'), 'index.html must load platform/tauri/bridge.js');
for (const relative of bridgeScripts) {
  vm.runInContext(
    fs.readFileSync(path.join(root, 'src', relative), 'utf8'),
    context,
    { filename: relative },
  );
}

const api = windowObject.TauriBridge;
assert.ok(api.available, 'desktop bridge must expose the real surface under a stubbed __TAURI__');

const combinedSnapshots = [];
const unsubscribeCombined = api.state.subscribeMany(['settings', 'models'], snapshot => {
  combinedSnapshots.push(snapshot);
});
const settingsSnapshots = [];
const unsubscribeSettings = api.state.subscribe('settings', snapshot => {
  settingsSnapshots.push(snapshot);
});

// First publication: the chat domain changes and both subscriptions receive
// their first snapshot.
api.chat.prefillComposer('draft-1');
assert.equal(combinedSnapshots.length, 1, 'subscribeMany must deliver the first publication');
assert.equal(settingsSnapshots.length, 1, 'subscribe must deliver the first publication');
assert.ok(Object.isFrozen(combinedSnapshots[0]), 'the combined snapshot must be frozen');

// The second round is still a chat-only publication: callbacks must fire as
// usual (no lost notifications), but unrelated domain identities must stay
// stable so React can skip the unrelated render.
api.chat.prefillComposer('draft-2');
assert.equal(combinedSnapshots.length, 2, 'callbacks must fire on every publication');
assert.equal(settingsSnapshots.length, 2, 'single-domain callbacks must fire on every publication');
assert.equal(combinedSnapshots[0], combinedSnapshots[1],
  'a chat-only publication must preserve an unrelated multi-domain combined snapshot identity');
assert.equal(settingsSnapshots[0], settingsSnapshots[1],
  'a chat-only publication must preserve an unrelated single-domain snapshot identity');

// A subscribed domain (models) really changes: the combined snapshot must get
// a new identity carrying the new value, and the unchanged settings slice keeps
// its identity inside the rebuilt combined object. loadSessionModel(null)
// publishes exactly one models-only update (currentSessionModelId +
// effectiveModelConfig) for the draft state.
invokeResponse = async command => (command === 'get_effective_model_config' ? { model: 'm1' } : null);
await api.models.loadSessionModel(null);
assert.equal(combinedSnapshots.length, 3, 'a subscribed-domain change must publish a new combined snapshot');
assert.notEqual(combinedSnapshots[1], combinedSnapshots[2],
  'a subscribed-domain change must rebuild the combined snapshot identity');
assert.equal(combinedSnapshots[2].effectiveModelConfig.model, 'm1',
  'the rebuilt combined snapshot must carry the changed models value');
assert.ok(Object.isFrozen(combinedSnapshots[2]), 'the rebuilt combined snapshot must be frozen');
assert.equal(combinedSnapshots[2].settings, combinedSnapshots[1].settings,
  'the unchanged settings slice must keep its identity inside the rebuilt combined snapshot');

// Single-revision publication semantics: every publication advances the same
// revision and re-materializes each domain slice together (host snapshot per
// publication); identity stability is guaranteed by subscriptionStateValue's
// structural comparison, not per-domain cache hits. A monitor-only
// publication therefore still reads the unchanged models source, but must
// keep two contracts: callbacks fire on every publication, and the combined
// snapshot identity stays unchanged when the content did not change (React
// skips unrelated renders via Object.is). (Open's shared-bridge refactor made
// the whole-list models reload a startup-internal loader, so the baseline is
// taken after draining the bridge's load-time auto-init publications — the
// coherent notify plus its fire-and-forget status/persona/scheduled loaders;
// the earlier model APIs publish synchronously and start nothing deferred.)
for (let attempt = 0; attempt < 5; attempt += 1) {
  await new Promise(resolve => { setTimeout(resolve, 0); });
}
const combinedSnapshotBeforeMonitor = combinedSnapshots.at(-1);
const combinedCallbackCountBeforeMonitor = combinedSnapshots.length;

const monitorSnapshot = {
  app: null,
  cpu: null,
  gpu: null,
  ram: null,
  self_perf: null,
  vllm: null,
};
invokeResponse = async command => command === 'get_monitor_snapshot' ? monitorSnapshot : {};
const monitorSnapshots = [];
const unsubscribeMonitor = api.state.subscribe('monitor', snapshot => {
  monitorSnapshots.push(snapshot);
});
api.monitor.startMonitorPolling();
for (let attempt = 0; attempt < 20
  && !monitorSnapshots.some(snapshot => snapshot.monitor?._fmt); attempt += 1) {
  await new Promise(resolve => { setTimeout(resolve, 0); });
}
assert.ok(monitorSnapshots.some(snapshot => snapshot.monitor?._fmt),
  'monitor polling must publish its first snapshot');
assert.equal(combinedSnapshots.length, combinedCallbackCountBeforeMonitor + 1,
  'monitor-only publication must preserve the every-publication callback contract');
assert.equal(combinedSnapshots.at(-1), combinedSnapshotBeforeMonitor,
  'monitor-only publication must preserve the unrelated models snapshot identity');

// Monitor owns the displayed snapshot, but a newly discovered context window
// also updates chat.tokens.max. The precise publication must invalidate both
// domains so the token progress denominator cannot become stale.
const chatSnapshots = [];
const unsubscribeChat = api.state.subscribe('chat', snapshot => {
  chatSnapshots.push(snapshot);
});
const monitorWithContextLimit = {
  ...monitorSnapshot,
  vllm: {
    configured_model: 'model-two',
    health_status: 'verified',
    max_model_len: 77777,
    metrics_applicable: false,
    model: 'model-two',
    status: 'ready',
    target_kind: 'local',
  },
};
invokeResponse = async command => command === 'get_monitor_snapshot' ? monitorWithContextLimit : {};
// Re-polls immediately (not via the interval): the vm's setInterval stub
// returns falsy 0, so startMonitorPolling's poller-dedupe guard never trips
// and each call runs its one synchronous pollMonitor().
api.monitor.startMonitorPolling();
for (let attempt = 0; attempt < 20
  && !chatSnapshots.some(snapshot => snapshot.tokens?.max === 77777); attempt += 1) {
  await new Promise(resolve => { setTimeout(resolve, 0); });
}
assert.ok(chatSnapshots.some(snapshot => snapshot.tokens?.max === 77777),
  'monitor context discovery must invalidate the chat token slice');

unsubscribeChat();
unsubscribeMonitor();
const combinedCountBeforeUnsubscribe = combinedSnapshots.length;
const settingsCountBeforeUnsubscribe = settingsSnapshots.length;
unsubscribeCombined();
unsubscribeSettings();
api.chat.prefillComposer('draft-3');
assert.equal(combinedSnapshots.length, combinedCountBeforeUnsubscribe, 'unsubscribe must stop subscribeMany deliveries');
assert.equal(settingsSnapshots.length, settingsCountBeforeUnsubscribe, 'unsubscribe must stop subscribe deliveries');

// Exercise the desktop public cold-load path with real replay and immutable
// subscriptions, including duplicate IDs and the cached return to that history.
const replayLoads = [];
invokeResponse = async (command, args) => {
  if (command !== 'load_session') return null;
  replayLoads.push(args.id);
  return {
    metadata: { id: args.id, title: args.id }, artifacts: [],
    messages: args.id === 'replay-A' ? [
      { role: 'user', content: [{ type: 'text', text: 'Restore tool history' }] },
      { role: 'assistant', content: [
        { type: 'tool_use', id: 'duplicate', name: 'Bash', input: { command: 'first' } },
        { type: 'tool_use', id: 'duplicate', name: 'Bash', input: { command: 'second' } },
        { type: 'tool_use', id: 'failure', name: 'Bash', input: { command: 'fail' } },
        { type: 'tool_use', id: 'pending', name: 'Bash', input: { command: 'pending' } },
      ] },
      { role: 'user', content: [
        { type: 'tool_result', tool_use_id: 'duplicate', content: 'complete' },
        { type: 'tool_result', tool_use_id: 'failure', content: 'failed', is_error: true },
      ] },
      { role: 'assistant', content: [{ type: 'text', text: 'Restored' }] },
    ] : [{ role: 'user', content: [{ type: 'text', text: 'Other session' }] }],
  };
};
let replaySnapshot;
const unsubscribeReplay = api.state.subscribe('chat', snapshot => { replaySnapshot = snapshot; });
assert.equal(await api.sessions.switchToSession('replay-A'), true);
const replayRows = () => Array.from(replaySnapshot.chatItems.filter(item => item.type === 'tool'), item => ({
  command: item.args.command, output: item.output, state: item.state, success: item.success,
}));
const expectedReplayRows = [
  { command: 'first', output: 'complete', state: 'done', success: true },
  { command: 'second', output: null, state: 'pending', success: null },
  { command: 'fail', output: 'failed', state: 'failed', success: false },
  { command: 'pending', output: null, state: 'failed', success: false },
];
assert.deepEqual(replayRows(), expectedReplayRows);
const firstReplaySnapshot = replaySnapshot;
assert.ok(Object.isFrozen(firstReplaySnapshot.chatItems));
assert.equal(await api.sessions.switchToSession('replay-B'), true);
assert.equal(await api.sessions.switchToSession('replay-A'), true);
assert.deepEqual(replayRows(), expectedReplayRows);
assert.deepEqual(replayLoads, ['replay-A', 'replay-B']);
assert.equal(firstReplaySnapshot.chatItems[1].output, 'complete', 'later navigation must not mutate an old published tool');
unsubscribeReplay();
