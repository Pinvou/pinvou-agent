import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const updaterSource = fs.readFileSync(
  path.join(here, '..', 'src', 'platform', 'tauri', 'bridge', 'updater.js'),
  'utf8',
);

function loadUpdaterFeature() {
  const listeners = new Map();
  const timers = new Map();
  let nextTimerId = 1;
  const root = {
    __PINVOU_TAURI_BRIDGE_FEATURES__: {},
    setTimeout(callback) {
      const id = nextTimerId++;
      timers.set(id, callback);
      return id;
    },
    clearTimeout(id) {
      timers.delete(id);
    },
  };
  vm.runInNewContext(updaterSource, { window: root });

  const state = {
    updateProgress: 0,
    updateDownloading: true,
    sessions: [],
  };
  const notifications = [];
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.updater;
  factory({
    state,
    notify() {
      notifications.push(state.updateProgress);
    },
    invoke() {
      return Promise.resolve(null);
    },
    listen(name, callback) {
      listeners.set(name, callback);
    },
    refreshHistoryList() {
      return Promise.resolve();
    },
    getBuffer() {},
    bt(key) {
      return key;
    },
  });

  return {
    state,
    notifications,
    emitProgress(downloaded, total) {
      listeners.get('update:progress')({ payload: { downloaded, total } });
    },
    flushTimers() {
      const callbacks = [...timers.values()];
      timers.clear();
      for (const callback of callbacks) callback();
    },
    pendingTimerCount() {
      return timers.size;
    },
  };
}

test('update progress coalesces burst events before notifying the React tree', () => {
  const runtime = loadUpdaterFeature();

  for (let downloaded = 1; downloaded <= 500; downloaded += 1) {
    runtime.emitProgress(downloaded, 1000);
  }

  assert.equal(runtime.state.updateProgress, 50, 'the latest progress must remain immediately readable');
  assert.equal(runtime.pendingTimerCount(), 1, 'a burst must schedule only one UI publication');
  assert.deepEqual(runtime.notifications, [], 'the burst must not synchronously re-render the app');

  runtime.flushTimers();
  assert.deepEqual(runtime.notifications, [50], 'the coalesced publication must expose the latest percentage');
});

test('completion flushes immediately and cancels a pending progress publication', () => {
  const runtime = loadUpdaterFeature();

  runtime.emitProgress(250, 1000);
  runtime.emitProgress(1000, 1000);

  assert.equal(runtime.state.updateProgress, 100);
  assert.equal(runtime.pendingTimerCount(), 0, 'completion must cancel the stale delayed publication');
  assert.deepEqual(runtime.notifications, [100], 'completion must publish immediately exactly once');

  runtime.flushTimers();
  assert.deepEqual(runtime.notifications, [100], 'a cancelled delayed publication must not fire later');
});
