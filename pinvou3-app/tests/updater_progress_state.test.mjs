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

function loadUpdaterFeature(options = {}) {
  const listeners = new Map();
  const timers = new Map();
  let nextTimerId = 1;
  const root = {
    __PINVOU_TAURI_BRIDGE_FEATURES__: {},
    setTimeout(callback, delay) {
      const id = nextTimerId++;
      timers.set(id, { callback, delay });
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
    updateCancelling: false,
    updateInfo: { available: true, platform: 'windows' },
    sessions: [],
    ...options.state,
  };
  const notifications = [];
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.updater;
  const updater = factory({
    state,
    notify() {
      notifications.push(state.updateProgress);
    },
    invoke(command, args) {
      return options.invoke ? options.invoke(command, args) : Promise.resolve(null);
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
    updater,
    state,
    notifications,
    emitProgress(downloaded, total) {
      listeners.get('update:progress')({ payload: { downloaded, total } });
    },
    flushTimers() {
      const callbacks = [...timers.values()].map(timer => timer.callback);
      timers.clear();
      for (const callback of callbacks) callback();
    },
    pendingTimerDelays() {
      return [...timers.values()].map(timer => timer.delay);
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
  assert.deepEqual(runtime.pendingTimerDelays(), [200], 'progress publication must use the 200 ms interval');
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

  runtime.emitProgress(900, 1000);
  runtime.flushTimers();
  assert.equal(runtime.state.updateProgress, 100, 'late progress after completion must be ignored');
  assert.deepEqual(runtime.notifications, [100], 'late progress after completion must not notify');
});

test('cancellation removes pending work and ignores late progress events', () => {
  const runtime = loadUpdaterFeature();

  runtime.emitProgress(250, 1000);
  runtime.updater.cancelUpdate();

  assert.equal(runtime.pendingTimerCount(), 0, 'cancellation must remove the pending progress publication');
  assert.deepEqual(runtime.notifications, [25], 'the cancellation state change must publish once');

  runtime.emitProgress(500, 1000);
  runtime.flushTimers();
  assert.equal(runtime.state.updateProgress, 25, 'progress arriving after cancellation must be ignored');
  assert.deepEqual(runtime.notifications, [25], 'late progress after cancellation must not notify');
});

test('download failure removes pending work and ignores late progress events', async () => {
  let rejectDownload;
  const download = new Promise((resolve, reject) => {
    rejectDownload = reject;
  });
  const runtime = loadUpdaterFeature({
    state: { updateDownloading: false },
    invoke(command) {
      return command === 'download_update' ? download : Promise.resolve(null);
    },
  });

  const result = runtime.updater.downloadAndInstallUpdate();
  runtime.emitProgress(250, 1000);
  assert.equal(runtime.pendingTimerCount(), 1);

  rejectDownload(new Error('network unavailable'));
  assert.equal(await result, false);
  assert.equal(runtime.pendingTimerCount(), 0, 'failure must remove the pending progress publication');
  assert.equal(runtime.state.updateDownloading, false);
  assert.match(runtime.state.updateError, /network unavailable/);

  const notificationsAfterFailure = [...runtime.notifications];
  runtime.emitProgress(500, 1000);
  runtime.flushTimers();
  assert.equal(runtime.state.updateProgress, 25, 'progress arriving after failure must be ignored');
  assert.deepEqual(runtime.notifications, notificationsAfterFailure, 'late progress after failure must not notify');
});
