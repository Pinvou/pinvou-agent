import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridgeSource = fs.readFileSync(path.join(root, 'src', 'platform', 'tauri', 'bridge.js'), 'utf8');
const detachedShellSource = fs.readFileSync(path.join(root, 'src', 'app', 'DetachedShell.jsx'), 'utf8');
const monitorViewSource = fs.readFileSync(path.join(root, 'src', 'features', 'monitor', 'MonitorView.jsx'), 'utf8');

function featureRegistry(calls) {
  const registry = {};
  for (const match of bridgeSource.matchAll(/installBridgeFeature\("([^"]+)"/g)) {
    const feature = match[1];
    registry[feature] = function () {
      return new Proxy({}, {
        get(_target, method) {
          return function (...args) {
            calls.push({ kind: 'feature', feature, method: String(method), args });
            if (feature === 'sessions' && method === 'switchToSession') return Promise.resolve(true);
            return Promise.resolve(undefined);
          };
        },
      });
    };
  }
  return registry;
}

async function initialize(search) {
  const calls = [];
  const windowObject = {
    __TAURI__: {
      core: {
        invoke(command, args) {
          calls.push({ kind: 'invoke', command, args });
          return Promise.resolve(command === 'get_platform_capabilities' ? {} : null);
        },
      },
      event: {
        listen(event) {
          calls.push({ kind: 'listen', event });
          return Promise.resolve(function () {});
        },
      },
      dialog: { open: async function () { return null; } },
    },
    __PINVOU_TAURI_BRIDGE_FEATURES__: featureRegistry(calls),
    location: { search },
    performance: { now: function () { return 0; } },
  };
  const context = vm.createContext({
    window: windowObject,
    document: { readyState: 'loading', addEventListener: function () {} },
    console,
    setInterval(fn, delay) {
      calls.push({ kind: 'interval', fn, delay });
      return 1;
    },
    clearInterval: function () {},
    setTimeout,
    clearTimeout,
    structuredClone,
    URL,
    Blob,
  });
  vm.runInContext(bridgeSource, context, { filename: 'tauri-bridge.js' });
  await windowObject.TauriBridge.lifecycle.init();
  return calls;
}

function featureCalls(calls, feature, method) {
  return calls.filter(call => call.kind === 'feature' && call.feature === feature && call.method === method);
}

const sessionCalls = await initialize('?detached=1&kind=session&id=session%2D42');
const historyIndex = sessionCalls.findIndex(call => call.kind === 'feature'
  && call.feature === 'sessions' && call.method === 'refreshHistoryList');
const switchIndex = sessionCalls.findIndex(call => call.kind === 'feature'
  && call.feature === 'sessions' && call.method === 'switchToSession');
assert.ok(historyIndex >= 0, 'detached session must load the durable session index');
assert.ok(switchIndex > historyIndex, 'detached session must bind its id after history initialization');
assert.equal(sessionCalls[switchIndex].args[0], 'session-42', 'detached session id must be URL-decoded');
assert.equal(featureCalls(sessionCalls, 'sessions', 'enterDraft').length, 0,
  'detached session initialization must not reset the target to a blank draft');
assert.equal(featureCalls(sessionCalls, 'scheduled', 'loadScheduledTasks').length, 0,
  'detached windows must not duplicate main-window scheduled summary polling');
assert.equal(featureCalls(sessionCalls, 'updater', 'checkForUpdateSilently').length, 0,
  'detached windows must not duplicate main-window update checks');
assert.equal(featureCalls(sessionCalls, 'remote-control', 'startDesktopProxy').length, 0,
  'detached windows must not own the desktop remote-control proxy');

const mainCalls = await initialize('');
assert.equal(featureCalls(mainCalls, 'sessions', 'enterDraft').length, 1,
  'main-window startup must retain lazy blank-draft behavior');
assert.equal(featureCalls(mainCalls, 'sessions', 'switchToSession').length, 0,
  'main-window startup must not inherit a detached target');
assert.equal(featureCalls(mainCalls, 'scheduled', 'loadScheduledTasks').length, 1,
  'main window must retain scheduled summary loading');
assert.equal(featureCalls(mainCalls, 'remote-control', 'startDesktopProxy').length, 1,
  'main window must retain desktop remote-control proxy ownership');

const domainMatch = detachedShellSource.match(/useBridgeState\(\[([^\]]+)\]\)/);
assert.ok(domainMatch, 'DetachedShell must subscribe through useBridgeState');
assert.match(domainMatch[1], /['"]monitor['"]/, 'detached monitor must receive monitor state updates');
assert.match(domainMatch[1], /['"]personas['"]/, 'detached card pool must receive persona state updates');
assert.doesNotMatch(detachedShellSource, /sessions\.switchToSession/,
  'DetachedShell must not race lifecycle initialization with a session-switch effect');
assert.doesNotMatch(detachedShellSource, /monitor\.startMonitorPolling/,
  'DetachedShell must not duplicate the monitor view polling lifecycle');
assert.match(monitorViewSource, /monitor\.startMonitorPolling/,
  'MonitorView must remain the single owner of monitor polling');
assert.match(monitorViewSource, /monitor\.stopMonitorPolling/,
  'MonitorView must stop polling when it unmounts');

console.log('detached window lifecycle tests passed');
