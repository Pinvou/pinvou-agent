import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { expectedWebBridgeApi } from './bridge_domain_contract.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const webBridgeRoot = path.join(root, 'src', 'platform', 'web');
const read = relativePath => fs.readFileSync(path.join(webBridgeRoot, relativePath), 'utf8');

const storage = new Map();
const localStorage = {
  getItem(key) { return storage.has(key) ? storage.get(key) : null; },
  setItem(key, value) { storage.set(key, String(value)); },
  removeItem(key) { storage.delete(key); },
};
const documentObject = {
  readyState: 'loading',
  addEventListener() {},
  createElement() {
    return { click() {}, remove() {}, style: {}, setAttribute() {} };
  },
  body: { appendChild() {} },
};
let invokeResponse = async () => null;
const windowObject = {
  PinvouPlatform: {
    kind: 'web',
    isWeb: true,
    capabilities: {},
    can: () => false,
    canInvoke: () => false,
  },
  __TAURI__: {
    core: { invoke: (...args) => invokeResponse(...args) },
    event: { listen: async () => function () {} },
    dialog: { open: async () => null },
  },
  location: { search: '', href: 'https://example.test/pinvou3/remote/' },
  localStorage,
  crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
  performance: { now: () => 0 },
  addEventListener() {},
  setTimeout,
  clearTimeout,
};
const nativeStructuredClone = globalThis.structuredClone;
let deepCloneCalls = 0;
const context = vm.createContext({
  window: windowObject,
  document: documentObject,
  navigator: { mediaDevices: null },
  localStorage,
  console,
  setTimeout,
  clearTimeout,
  setInterval,
  clearInterval,
  structuredClone(value) {
    deepCloneCalls += 1;
    return nativeStructuredClone(value);
  },
  URL,
  URLSearchParams,
  Blob,
  Uint8Array,
  ArrayBuffer,
  TextEncoder,
  TextDecoder,
});

vm.runInContext(read('bridge.js'), context, { filename: 'platform/web/bridge.js' });
const flat = windowObject.TauriBridge;
assert.equal(typeof flat.getState, 'function', 'Web transport must expose its private flat state before adaptation');

let snapshotReads = 0;
const readFlatState = flat.getState;
flat.getState = function () {
  snapshotReads += 1;
  return readFlatState();
};

vm.runInContext(read('bridge/domain-adapter.js'), context, { filename: 'platform/web/bridge/domain-adapter.js' });
const api = windowObject.TauriBridge;
const expectedApi = expectedWebBridgeApi();

assert.deepEqual(Object.keys(api).sort(), ['available', ...Object.keys(expectedApi)].sort()); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of string arrays matches assertion expectations
for (const [domain, methods] of Object.entries(expectedApi)) {
  assert.deepEqual(Object.keys(api[domain]).sort(), [...methods].sort(), `${domain} Web API surface changed`); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of string arrays matches assertion expectations
}
assert.equal(api.getState, undefined, 'Web flat compatibility facade must stay private');
assert.equal(api.sendMessage, undefined, 'Web flat command facade must stay private');

snapshotReads = 0;
const state = api.state.getMany(['sessions', 'settings']);
assert.equal(snapshotReads, 1, 'getMany must derive all slices from one consistent state snapshot');
assert.ok(Object.hasOwn(state, 'sessions'));
assert.ok(Object.hasOwn(state, 'settings'));
assert.equal(Object.hasOwn(state, 'messages'), false);
assert.throws(() => api.state.get('unknown'), /Unknown Tauri bridge state slice/);

const flatSnapshots = [];
const secondFlatSnapshots = [];
const chatSnapshots = [];
const combinedSnapshots = [];
const unsubscribeFlat = flat.subscribe(snapshot => { flatSnapshots.push(snapshot); });
const unsubscribeSecondFlat = flat.subscribe(snapshot => { secondFlatSnapshots.push(snapshot); });
const unsubscribeChat = api.state.subscribe('chat', snapshot => { chatSnapshots.push(snapshot); });
const unsubscribeCombined = api.state.subscribeMany(['sessions', 'chat'], snapshot => { combinedSnapshots.push(snapshot); });
snapshotReads = 0;
deepCloneCalls = 0;
invokeResponse = async command => command === 'web_access_ingest_file'
  ? { basename: 'stable-snapshot.txt', handle: 'attachment-handle' }
  : null;
await api.attachments.addAttachmentByPath('/tmp/stable-snapshot.txt');
assert.equal(snapshotReads, 0, 'subscription notifications must use the supplied transport snapshot');
assert.equal(deepCloneCalls, 0, 'subscription notifications must not deep-clone the transcript');
assert.equal(flatSnapshots.length, 2, 'Web flat subscribers should observe parsing and ready updates');
assert.equal(chatSnapshots.length, 2, 'Web domain subscribers should observe parsing and ready updates');
assert.equal(combinedSnapshots.length, 2, 'Web multi-domain subscribers should observe parsing and ready updates');
assert.equal(flatSnapshots[0], secondFlatSnapshots[0],
  'Web flat subscribers in one revision should receive the exact same immutable snapshot');
assert.equal(flatSnapshots[1], secondFlatSnapshots[1],
  'Web flat subscribers should continue sharing the same snapshot in later revisions');
assert.equal(flatSnapshots[1].attachments, chatSnapshots[1].attachments,
  'Web flat and domain subscribers should share the same changed domain subtree in one revision');
assert.equal(flatSnapshots[1].attachments, combinedSnapshots[1].attachments,
  'Web flat and multi-domain subscribers should share the same domain subtree in one revision');
assert.equal(flatSnapshots[0].messages, flatSnapshots[1].messages,
  'unchanged Web transcript subtrees should be structurally shared across revisions');
assert.equal(flatSnapshots[1].messages, chatSnapshots[1].messages,
  'Web flat and domain subscribers should share unchanged transcript subtrees in one revision');
assert.equal(flatSnapshots[0].attachments[0].status, 'parsing');
assert.equal(flatSnapshots[1].attachments[0].status, 'ready');
assert.equal(chatSnapshots[0].attachments[0].status, 'parsing');
assert.equal(chatSnapshots[1].attachments[0].status, 'ready');
assert.ok(Object.isFrozen(flatSnapshots[0].attachments[0]));
assert.ok(Object.isFrozen(chatSnapshots[0].attachments[0]));
assert.equal(Reflect.set(flatSnapshots[0].attachments[0], 'status', 'subscriber-only'), false,
  'a flat subscriber must not mutate a nested item');
assert.equal(Reflect.set(chatSnapshots[1].attachments[0].result, 'handle', 'subscriber-only'), false,
  'a domain subscriber must not mutate a nested result');
assert.equal(flatSnapshots[0].attachments[0].status, 'parsing', 'an older flat snapshot must remain stable');
assert.equal(chatSnapshots[0].attachments[0].status, 'parsing', 'an older domain snapshot must remain stable');
assert.equal(secondFlatSnapshots[0].attachments[0].status, 'parsing',
  'one flat subscriber must not affect a second subscriber');
assert.equal(combinedSnapshots[1].attachments[0].result.handle, 'attachment-handle',
  'one domain subscriber must not affect another domain subscriber');
assert.equal(api.state.get('chat').attachments[0].result.handle, 'attachment-handle',
  'subscription writes must not mutate Web bridge state');
unsubscribeFlat();
unsubscribeSecondFlat();
unsubscribeChat();
unsubscribeCombined();

const flatReentrantFirst = [];
const flatReentrantSecond = [];
const domainReentrantFirst = [];
const domainReentrantSecond = [];
const unsubscribeFlatReentrantFirst = flat.subscribe(snapshot => {
  const text = snapshot.composerPrefill.text;
  flatReentrantFirst.push(text);
  if (text === 'outer') api.chat.prefillComposer('nested');
});
const unsubscribeFlatReentrantSecond = flat.subscribe(snapshot => {
  flatReentrantSecond.push(snapshot.composerPrefill.text);
});
const unsubscribeDomainReentrantFirst = api.state.subscribe('chat', snapshot => {
  domainReentrantFirst.push(snapshot.composerPrefill.text);
});
const unsubscribeDomainReentrantSecond = api.state.subscribe('chat', snapshot => {
  domainReentrantSecond.push(snapshot.composerPrefill.text);
});
api.chat.prefillComposer('outer');
assert.deepEqual(flatReentrantFirst, ['outer', 'nested']);
assert.deepEqual(flatReentrantSecond, ['outer', 'nested'], 'Web flat subscribers must receive outer before nested');
assert.deepEqual(domainReentrantFirst, ['outer', 'nested']);
assert.deepEqual(domainReentrantSecond, ['outer', 'nested'], 'Web domain subscribers must receive outer before nested');
assert.equal(api.state.get('chat').composerPrefill.text, 'nested');
unsubscribeFlatReentrantFirst();
unsubscribeFlatReentrantSecond();
unsubscribeDomainReentrantFirst();
unsubscribeDomainReentrantSecond();

const membershipFirst = [];
const membershipSecond = [];
const membershipAdded = [];
// eslint-disable-next-line prefer-const -- reassigned at line 200
let unsubscribeMembershipSecond;
let unsubscribeMembershipAdded = () => {};
const unsubscribeMembershipFirst = flat.subscribe(snapshot => {
  const text = snapshot.composerPrefill.text;
  membershipFirst.push(text);
  if (text === 'membership-outer') {
    unsubscribeMembershipSecond();
    unsubscribeMembershipAdded = flat.subscribe(next => { membershipAdded.push(next.composerPrefill.text); });
    api.chat.prefillComposer('membership-nested');
  }
});
unsubscribeMembershipSecond = flat.subscribe(snapshot => { membershipSecond.push(snapshot.composerPrefill.text); });
api.chat.prefillComposer('membership-outer');
assert.deepEqual(membershipFirst, ['membership-outer', 'membership-nested']);
assert.deepEqual(membershipSecond, ['membership-outer'],
  'Web unsubscribe during a round should affect only later queued rounds');
assert.deepEqual(membershipAdded, ['membership-nested'],
  'Web subscribe during a round should affect only later queued rounds');
unsubscribeMembershipFirst();
unsubscribeMembershipAdded();

const settingsSnapshots = [];
let negativeZero = true;
const settingsResult = () => {
  const result = JSON.parse('{"language":"en","__proto__":{"marker":"own-value"}}');
  Object.defineProperty(result, 'nan', { enumerable: true, value: NaN, writable: true });
  result.zero = negativeZero ? -0 : 0;
  return result;
};
const unsubscribeSettings = api.state.subscribe('settings', snapshot => { settingsSnapshots.push(snapshot); });
invokeResponse = async command => command === 'web_access_update_settings' ? settingsResult() : null;
assert.equal(await api.settings.saveSettings({ language: 'en' }), true);
api.chat.prefillComposer('same-settings-revision');
const firstSettings = settingsSnapshots[0];
const repeatedSettings = settingsSnapshots[1];
assert.ok(Object.hasOwn(firstSettings.settings, '__proto__'));
assert.equal(firstSettings.settings.__proto__.marker, 'own-value');
assert.equal(Object.getPrototypeOf(firstSettings.settings), Object.getPrototypeOf(firstSettings));
assert.equal(Object.getPrototypeOf(firstSettings.settings).marker, undefined);
assert.equal(firstSettings.settings, repeatedSettings.settings, 'Object.is should reuse a subtree containing NaN');
assert.ok(Number.isNaN(firstSettings.settings.nan));
assert.ok(Object.is(firstSettings.settings.zero, -0));
negativeZero = false;
assert.equal(await api.settings.saveSettings({ language: 'en' }), true);
const changedSettings = settingsSnapshots.at(-1);
assert.notEqual(changedSettings.settings, repeatedSettings.settings, 'Object.is should distinguish -0 from +0');
assert.ok(Object.is(changedSettings.settings.zero, 0));
assert.ok(Object.prototype.hasOwnProperty.call(changedSettings.settings, '__proto__'),
  'web copy-on-write updates should retain an existing own __proto__ value');
assert.equal(changedSettings.settings['__proto__'].marker, 'own-value');
assert.equal(Object.getPrototypeOf(changedSettings.settings).marker, undefined,
  'web copy-on-write updates must not route __proto__ through the prototype setter');
unsubscribeSettings();

const memorySources = {
  profile: { available: true }, preferences: { available: true }, work_context: { available: true },
  current_focus: { available: true }, recent_activity: { available: true }, recent_work: { available: true },
  pending: { available: true }, never: { available: true }, runtime: { available: true }, snapshot: { available: true },
};
const memoryOverview = overrides => ({
  profile: null, preferences: [], work_context: [], current_focus: [], recent_activity: [],
  recent_work: [], pending: [], never: [], runtime: null, snapshot_path: '', warnings: [],
  sources: memorySources, ...(overrides || {}),
});
invokeResponse = async command => command === 'get_memory_overview'
  ? memoryOverview({ preferences: [{ id: 'web-pref-old', text: 'old' }], work_context: [{ id: 'web-ctx-old', text: 'old' }] })
  : null;
await api.memory.loadMemoryOverview();
const preferenceCleanup = { code: 'memory_topic_cleanup_required', source: 'preferences', detail: 'occupied' };
invokeResponse = async command => command === 'update_memory_preference'
  ? { value: { id: 'web-pref-new', text: 'new' }, runtime: null, warnings: [{ code: 'runtime_refresh_failed' }, preferenceCleanup] }
  : memoryOverview({
      preferences: [{ id: 'web-pref-new', text: 'new' }],
      work_context: [{ id: 'web-ctx-old', text: 'old' }],
      warnings: [{ code: 'snapshot_refresh_failed' }, preferenceCleanup],
    });
await api.memory.updateMemoryItem('preference', 'web-pref-old', { topic: 'workflow_preference' });
let memoryState = api.state.get('memory').memory;
assert.deepEqual(memoryState.preferences.map(item => item.id), ['web-pref-new']);
assert.equal(memoryState.warnings[0].code, 'memory_topic_cleanup_required');

const contextCleanup = { code: 'memory_topic_cleanup_required', source: 'work_context', detail: 'occupied' };
invokeResponse = async command => command === 'update_work_context_memory'
  ? { value: { id: 'web-ctx-new', text: 'new' }, runtime: null, warnings: [contextCleanup] }
  : memoryOverview({
      preferences: [{ id: 'web-pref-new', text: 'new' }],
      work_context: [{ id: 'web-ctx-new', text: 'new' }],
      warnings: [contextCleanup],
    });
await api.memory.updateMemoryItem('work_context', 'web-ctx-old', { topic: 'project_context' });
memoryState = api.state.get('memory').memory;
assert.deepEqual(memoryState.work_context.map(item => item.id), ['web-ctx-new']);
assert.equal(memoryState.warnings[0].code, 'memory_topic_cleanup_required');

const indexSource = fs.readFileSync(path.join(root, 'src', 'index.html'), 'utf8');
assert.ok(
  indexSource.indexOf('shared/bridge-messages.js') < indexSource.indexOf('platform/web/bridge.js'),
  'shared bridge messages must load before the web bridge',
);
assert.ok(
  indexSource.indexOf('shared/chunked-file-upload.js') < indexSource.indexOf('platform/web/bridge.js'),
  'the shared chunk uploader must load before platform bridges',
);
assert.ok(
  indexSource.indexOf('platform/web/bridge/turn-terminal.js') < indexSource.indexOf('platform/web/bridge.js'),
  'web turn terminal support must load before the web bridge',
);
assert.ok(
  indexSource.indexOf('platform/web/bridge.js') < indexSource.indexOf('platform/web/bridge/domain-adapter.js'),
  'Web domain adapter must load after the flat transport',
);
assert.ok(
  indexSource.indexOf('shared/bridge-messages.js') < indexSource.indexOf('platform/tauri/bridge/chat-events.js'),
  'shared bridge messages must load before the tauri bridge chat events',
);
assert.ok(
  indexSource.indexOf('shared/bridge-messages.js') < indexSource.indexOf('platform/tauri/bridge.js'),
  'shared bridge messages must load before the tauri bridge',
);

const stableCombined = [];
const unsubscribeStableCombined = api.state.subscribeMany(['sessions', 'chat'], snapshot => { stableCombined.push(snapshot); });
api.chat.prefillComposer('stable-combined-identity');
api.chat.removeQueued('no-such-queued-id');
assert.equal(stableCombined.length, 2, 'no-change notifications must still reach multi-domain subscribers');
assert.equal(stableCombined[0], stableCombined[1],
  'multi-domain combined slices must reuse identity when the transport snapshot is unchanged');
assert.ok(Object.isFrozen(stableCombined[0]), 'the reused combined slice must stay frozen');
api.chat.prefillComposer('stable-combined-identity-2');
assert.equal(stableCombined.length, 3);
assert.notEqual(stableCombined[1], stableCombined[2], 'a real state change must produce a new combined slice');
assert.equal(stableCombined[2].composerPrefill.text, 'stable-combined-identity-2');
unsubscribeStableCombined();

invokeResponse = async command => command === 'web_access_ingest_file' ? new Date(0) : null;
await assert.rejects(api.attachments.addAttachmentByPath('/tmp/non-plain.txt'), /only supports arrays and plain objects/);
const nonPlainAttachment = flat.getState().attachments.at(-1);
api.attachments.removeAttachment(nonPlainAttachment.id);
const cyclic = { value: 'cycle' };
cyclic.self = cyclic;
invokeResponse = async command => command === 'web_access_ingest_file' ? cyclic : null;
await assert.rejects(api.attachments.addAttachmentByPath('/tmp/cyclic.txt'), /must not contain cycles/);

// probeLocalServerKind 降级契约（PR #218 五审 P2）：web 桥层不得吞错伪造成
// generic——命令失败（web 白名单不含该命令/老版本桌面）必须 reject，由消费方
// （SettingsView）catch 后置 null 走 localProbeTiersForKind 默认四档；否则本地
// vLLM/Ollama 会被误报成「该端点不支持思考档位调节」。
invokeResponse = async command => {
  if (command !== 'probe_local_server_kind') return null;
  throw new Error('probe_local_server_kind is not allowed');
};
await assert.rejects(
  () => api.models.probeLocalServerKind('http://127.0.0.1:8000/v1'),
  /not allowed/,
  'web probeLocalServerKind must reject (not swallow) command failures',
);
invokeResponse = async command => (command === 'probe_local_server_kind' ? 'ollama' : null);
assert.equal(
  await api.models.probeLocalServerKind('http://127.0.0.1:11434/v1'),
  'ollama',
  'web probeLocalServerKind must pass the probed kind through unchanged',
);

console.log('web bridge domain contract passed');
