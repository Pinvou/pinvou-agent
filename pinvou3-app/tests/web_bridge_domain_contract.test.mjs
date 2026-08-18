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
  structuredClone,
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

assert.deepEqual(Object.keys(api).sort(), ['available', ...Object.keys(expectedApi)].sort());
for (const [domain, methods] of Object.entries(expectedApi)) {
  assert.deepEqual(Object.keys(api[domain]).sort(), [...methods].sort(), `${domain} Web API surface changed`);
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
  indexSource.indexOf('shared/model-service-errors.js') < indexSource.indexOf('shared/bridge-messages.js'),
  'model service error classifier must load before shared bridge messages',
);
assert.ok(
  indexSource.indexOf('shared/bridge-messages.js') < indexSource.indexOf('platform/web/bridge.js'),
  'shared bridge messages must load before the web bridge',
);
assert.ok(
  indexSource.indexOf('platform/web/bridge/turn-terminal.js') < indexSource.indexOf('platform/web/bridge.js'),
  'web turn terminal support must load before the web bridge',
);
assert.ok(
  indexSource.indexOf('platform/web/bridge.js') < indexSource.indexOf('platform/web/bridge/domain-adapter.js'),
  'Web domain adapter must load after the flat transport',
);

console.log('web bridge domain contract passed');
