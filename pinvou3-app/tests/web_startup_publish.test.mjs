import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

// Startup publish contract on the web bridge, mirroring
// detached_window_lifecycle.test.mjs on the tauri side: the startup loaders
// run in parallel, so with a slow settings backend the other loaders finish
// first. Without startup batching the bridge would publish partial snapshots
// while loadSettings is still in flight — before it writes, the settings
// slice stays at its structural initial value (null). The publish must be
// deferred until every loader has settled, so the FIRST snapshot a
// subscriber observes carries the slow loader's written payload.
// Post-startup updates must continue publishing immediately.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const webBridgeRoot = path.join(root, 'src', 'platform', 'web');

const storage = new Map();
const SLOW_SETTINGS = { language: 'zh-Hans', color_scheme: 'system', theme: 'genesis' };
const windowObject = {
  PinvouPlatform: { kind: 'web', isWeb: true, capabilities: {}, can: () => false, canInvoke: () => false },
  __TAURI__: {
    core: {
      // Deterministic interleaving: every command resolves null immediately
      // (each self-catching loader falls back), except get_settings which
      // settles last via a real timer with a recognizable payload.
      invoke(command) {
        if (command === 'get_settings') {
          return new Promise(resolve => { setTimeout(() => resolve(SLOW_SETTINGS), 20); });
        }
        return Promise.resolve(null);
      },
    },
    event: { listen: async () => () => {} },
    dialog: { open: async () => null },
  },
  location: { search: '', href: 'https://example.test/pinvou3/remote/' },
  addEventListener() {},
  removeEventListener() {},
  crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
  performance: { now: () => 0 },
};
const context = vm.createContext({
  window: windowObject,
  document: { readyState: 'loading', addEventListener() {} },
  navigator: { mediaDevices: null },
  localStorage: {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
    removeItem(key) { storage.delete(key); },
  },
  console,
  setTimeout,
  clearTimeout,
  // The 10s status poll and session retention timers stay inert — one fewer
  // source of post-startup publications inside the measured window.
  setInterval: () => 0,
  clearInterval() {},
  structuredClone,
  URL,
  URLSearchParams,
  Blob,
  Uint8Array,
  ArrayBuffer,
  TextEncoder,
  TextDecoder,
});

vm.runInContext(fs.readFileSync(path.join(root, 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8'), context, { filename: 'shared/bridge-shared-helpers.js' });
vm.runInContext(fs.readFileSync(path.join(webBridgeRoot, 'bridge.js'), 'utf8'), context, { filename: 'platform/web/bridge.js' });
vm.runInContext(fs.readFileSync(path.join(webBridgeRoot, 'bridge', 'domain-adapter.js'), 'utf8'), context, { filename: 'platform/web/bridge/domain-adapter.js' });

const flat = windowObject.TauriBridge;
assert.equal(typeof flat.lifecycle?.init, 'function',
  'web bridge must expose lifecycle.init for the startup contract');
assert.equal(typeof flat.settings.saveSettings, 'function',
  'web bridge must expose settings.saveSettings for the post-startup update');
assert.equal(typeof flat.scheduled.loadScheduledTasks, 'function',
  'web bridge must expose scheduled.loadScheduledTasks for the post-startup update');

const startupSnapshots = [];
const unsubscribeStartup = flat.state.subscribeMany(
  ['settings', 'models'],
  snapshot => startupSnapshots.push(snapshot),
);

await flat.lifecycle.init();
unsubscribeStartup();

// Coherency: snapshot #1 is the startup batch publish (init's finally), which
// must exist and must already carry the slow settings loader's payload —
// before loadSettings writes, state.settings stays null, so any partial
// per-loader publication would surface here as settings === null.
assert.ok(startupSnapshots.length >= 1, 'startup must publish at least one snapshot');
const firstSnapshot = startupSnapshots[0];
assert.ok(firstSnapshot.settings && firstSnapshot.settings.language === 'zh-Hans'
  && firstSnapshot.settings.color_scheme === 'system',
  'the first startup snapshot must carry the settings loader payload, not a half-initialized state');
assert.ok('savedModels' in firstSnapshot && 'activeModelId' in firstSnapshot,
  'the startup snapshot must carry models-domain loader state');

// Post-startup updates must publish immediately (per-detached-window contract
// on the tauri side). Web commands may publish several sequential rounds per
// call (the scheduled loader publishes begin/loading/end states), so pin the
// delta rather than an absolute count.
const afterSnapshots = [];
const unsubscribeAfter = flat.state.subscribe('settings', snapshot => afterSnapshots.push(snapshot));
await flat.scheduled.loadScheduledTasks();
const scheduledCount = afterSnapshots.length;
assert.ok(scheduledCount >= 1, 'scheduled load after startup must reach settings subscribers');
await flat.settings.saveSettings({ language: 'en' });
assert.ok(afterSnapshots.length > scheduledCount,
  'web state updates after startup must continue publishing immediately');
unsubscribeAfter();
