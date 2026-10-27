import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';
import { dict } from './helpers/i18n-all.js';
import { updateErrorCopy } from '../src/features/settings/update-check-copy.js';

function updater(invoke, initial = {}) {
  const window = {};
  const context = vm.createContext({ window });
  for (const path of ['../src/shared/bridge-shared-helpers.js', '../src/platform/tauri/bridge/updater.js']) {
    vm.runInContext(readFileSync(new URL(path, import.meta.url), 'utf8'), context, { filename: path });
  }
  const state = { updateInfo: null, updateCheckError: null, ...initial };
  let notifications = 0;
  const feature = window.__PINVOU_TAURI_BRIDGE_FEATURES__.updater({
    state, invoke, notify: () => { notifications += 1; },
  });
  return { state, feature, notifications: () => notifications };
}

test('real updater bridge keeps unsupported startup silent and manual status truthful', async () => {
  const calls = [];
  const invoke = async command => {
    calls.push(command);
    // Tauri rejects with the serialized Rust error string.
    // eslint-disable-next-line prefer-promise-reject-errors
    return Promise.reject('UPDATER_CHECK_UNSUPPORTED');
  };
  const run = updater(invoke, { appVersion: '1.2.3' });
  await run.feature.checkForUpdateSilently();
  assert.equal(run.state.updateCheckError, null);
  assert.equal(run.notifications(), 0);
  assert.equal(run.state.appVersion, '1.2.3');
  run.state.updateInfo = { available: true, latest_version: 'stale' };
  await run.feature.checkForUpdate();
  assert.deepEqual(calls, ['check_for_update', 'check_for_update']);
  assert.equal(run.state.updateCheckError, 'UPDATER_CHECK_UNSUPPORTED');
  assert.equal(run.state.updateInfo, null);
  assert.equal(run.state.updateChecking, false);
  for (const language of ['zh', 'en', 'ja']) {
    const message = updateErrorCopy(run.state, dict[language]);
    assert.equal(message, dict[language].updateCheckUnsupported);
    assert.notEqual(message, dict[language].upToDate);
    assert.ok(message.length > 0);
  }
});

test('manual check preserves genuine success and generic failure semantics', async () => {
  const current = updater(async command => {
    assert.equal(command, 'check_for_update');
    return { available: false, current_version: '1.2.3' };
  });
  await current.feature.checkForUpdate();
  assert.equal(current.state.updateCheckError, 'latest');
  assert.equal(current.state.appVersion, '1.2.3');
  const failed = updater(async command => {
    assert.equal(command, 'check_for_update');
    throw new Error('network offline');
  });
  await failed.feature.checkForUpdate();
  assert.equal(failed.state.updateCheckError, 'Error: network offline');
  assert.equal(updateErrorCopy(failed.state, dict.en), dict.en.updateCheckFailed);
  assert.notEqual(failed.state.updateCheckError, 'latest');
});
