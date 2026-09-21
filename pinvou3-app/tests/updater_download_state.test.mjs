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

// Slim revival of the former updater_progress_state.test.mjs coverage: the
// 200 ms progress-coalescing machinery it pinned was removed alongside the
// `update:progress` event (the Rust backend never emitted it), but the
// download/install state machine on the tauri updater domain still needs a
// direct unit pin for its failure, cancel, and completion transitions.
function loadUpdaterFeature(options = {}) {
  const root = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  vm.runInNewContext(updaterSource, { window: root });

  const state = {
    updateProgress: 0,
    updateDownloading: false,
    updateCancelling: false,
    updateError: null,
    updateReady: false,
    updateInfo: { available: true, platform: 'linux' },
    ...options.state,
  };
  const notifications = [];
  const invokeCalls = [];
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.updater;
  const updater = factory({
    state,
    notify() {
      notifications.push(state.updateProgress);
    },
    invoke(command, args) {
      invokeCalls.push({ command, args });
      return options.invoke ? options.invoke(command, args) : Promise.resolve(null);
    },
  });

  return { updater, state, notifications, invokeCalls };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

test('downloadAndInstallUpdate guards against missing or already-running updates', async () => {
  const missing = loadUpdaterFeature({ state: { updateInfo: null } });
  assert.equal(await missing.updater.downloadAndInstallUpdate(), false);
  assert.equal(missing.invokeCalls.length, 0, 'no backend command may run without update info');

  const unavailable = loadUpdaterFeature({ state: { updateInfo: { available: false, platform: 'linux' } } });
  assert.equal(await unavailable.updater.downloadAndInstallUpdate(), false);
  assert.equal(unavailable.invokeCalls.length, 0);

  const busy = loadUpdaterFeature({ state: { updateDownloading: true } });
  assert.equal(await busy.updater.downloadAndInstallUpdate(), false);
  assert.equal(busy.invokeCalls.length, 0);
});

test('a failed download surfaces updateError and clears the downloading flag', async () => {
  const runtime = loadUpdaterFeature({
    invoke: (command) => command === 'download_update'
      ? Promise.reject(new Error('network unavailable'))
      : Promise.resolve(null),
  });

  const installed = await runtime.updater.downloadAndInstallUpdate();

  assert.equal(installed, false);
  assert.match(String(runtime.state.updateError), /network unavailable/);
  assert.equal(runtime.state.updateDownloading, false);
  assert.equal(runtime.state.updateCancelling, false);
  assert.equal(runtime.state.updateReady, false, 'a failed download must not mark the update ready');
  assert.ok(!runtime.invokeCalls.some(call => call.command === 'install_update'),
    'a failed download must not reach the install step');
  assert.ok(!runtime.invokeCalls.some(call => call.command === 'restart_app'),
    'a failed download must not restart the app');
});

test('a cancelled download resets progress without reporting an error', async () => {
  const downloadGate = deferred();
  const runtime = loadUpdaterFeature({
    invoke: (command) => {
      if (command === 'download_update') return downloadGate.promise;
      if (command === 'cancel_download') return Promise.resolve(null);
      return Promise.resolve(null);
    },
  });

  const pending = runtime.updater.downloadAndInstallUpdate();
  await Promise.resolve();
  assert.equal(runtime.state.updateDownloading, true);
  assert.equal(runtime.state.updateProgress, 0);

  runtime.updater.cancelUpdate();
  assert.equal(runtime.state.updateCancelling, true, 'cancelling must raise the flag before the backend acks');
  assert.ok(runtime.invokeCalls.some(call => call.command === 'cancel_download'),
    'cancelling must interrupt the backend download loop');

  downloadGate.reject(new Error('已取消下载'));
  const installed = await pending;
  assert.equal(installed, false);
  assert.equal(runtime.state.updateProgress, 0, 'a cancelled download must reset progress');
  assert.equal(runtime.state.updateError, null, 'a user-initiated cancel is not an error');
  assert.equal(runtime.state.updateDownloading, false);
  assert.equal(runtime.state.updateCancelling, false);
});

test('a completed download installs, marks ready, and restarts on non-windows platforms', async () => {
  const runtime = loadUpdaterFeature({
    state: { updateInfo: { available: true, platform: 'linux' } },
  });

  const installed = await runtime.updater.downloadAndInstallUpdate();

  assert.equal(installed, true);
  assert.equal(runtime.state.updateProgress, 100);
  assert.equal(runtime.state.updateReady, true);
  assert.equal(runtime.state.updateError, null);
  assert.equal(runtime.state.updateDownloading, false);
  const install = runtime.invokeCalls.find(call => call.command === 'install_update');
  assert.ok(install, 'completion must run the install step');
  assert.equal(install.args.debPath, null, 'a bare download result is forwarded as the deb path');
  assert.ok(runtime.invokeCalls.some(call => call.command === 'restart_app'),
    'linux and macos restart the frontend after install');
});

test('a completed windows install starts no frontend restart', async () => {
  const runtime = loadUpdaterFeature({
    state: { updateInfo: { available: true, platform: 'windows' } },
    invoke: (command) => command === 'download_update'
      ? Promise.resolve({ installer_path: '/tmp/pinvou/update.msi' })
      : Promise.resolve(null),
  });

  const installed = await runtime.updater.downloadAndInstallUpdate();

  assert.equal(installed, true);
  const install = runtime.invokeCalls.find(call => call.command === 'install_update');
  assert.equal(install.args.installerPath, '/tmp/pinvou/update.msi',
    'a download result with installer_path must drive the installer channel');
  assert.ok(!runtime.invokeCalls.some(call => call.command === 'restart_app'),
    'windows exits through the launched installer instead of a frontend restart');
});
