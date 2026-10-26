import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { dict } from './helpers/i18n-all.js';
import { can } from '../src/shared/platform.js';
import {
  TRUSTED_TIME_STATES, canRefreshTrustedTime, createTrustedTimeDiagnostics,
  parseTrustedTimeStatus, trustedTimeErrorCode,
} from '../src/features/updater/trusted-time-diagnostics.js';

const status = (state = 'unknown', refreshing = false) => ({ protocolVersion: 1, state, refreshing });
function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

test('only exact host status DATA is displayed; accepted and Known are not status states', () => {
  for (const state of TRUSTED_TIME_STATES) assert.equal(parseTrustedTimeStatus(status(state)).state, state);
  for (const value of [null, [], {}, { ...status(), extra: true }, { ...status(), refreshing: 1 },
    { ...status(), protocolVersion: 2 }, status('known'), status('accepted')]) {
    assert.throws(() => parseTrustedTimeStatus(value));
  }
  assert.equal(canRefreshTrustedTime(status('cleanup_pending')), true);
  assert.equal(parseTrustedTimeStatus(status('cleanup_complete')).state, 'cleanup_complete');
  assert.equal(canRefreshTrustedTime(status('cleanup_complete')), true);
  assert.equal(canRefreshTrustedTime(status('cleanup_complete', true)), false);
  assert.equal(canRefreshTrustedTime(status('unknown', true)), false);
  assert.equal(canRefreshTrustedTime(status('integrity_fault')), true);
  assert.equal(canRefreshTrustedTime(status('integrity_fault', true)), false);
  for (const state of ['unsupported', 'setup_required', 'missing_policy']) {
    assert.equal(canRefreshTrustedTime(status(state)), false);
  }
});

test('first read is local only, and concurrent requests share one lane', async () => {
  const pending = deferred();
  let reads = 0;
  let refreshes = 0;
  const views = [];
  const ui = createTrustedTimeDiagnostics({
    inspectTrustedTime() { reads += 1; return pending.promise; },
    requestTrustedTimeRefresh() { refreshes += 1; },
  }, view => views.push(view));
  const first = ui.read();
  assert.equal(ui.read(), first);
  assert.equal(ui.refresh(), first);
  await Promise.resolve();
  assert.equal(reads, 1);
  assert.equal(refreshes, 0);
  pending.resolve(status());
  await first;
  assert.equal(views.at(-1).status.state, 'unknown');
  assert.equal(views.at(-1).busy, false);
  ui.dispose();
});

test('integrity fault recovery requires an explicit refresh and a new native observation', async () => {
  let state = 'integrity_fault';
  const calls = [];
  const views = [];
  const ui = createTrustedTimeDiagnostics({
    async inspectTrustedTime() { calls.push('read'); return status(state); },
    async requestTrustedTimeRefresh() { calls.push('refresh'); return { protocolVersion: 1, state: 'accepted' }; },
  }, view => views.push(view));
  await ui.read();
  assert.deepEqual(calls, ['read']);
  assert.equal(canRefreshTrustedTime(views.at(-1).status), true);
  await ui.refresh();
  assert.deepEqual(calls, ['read', 'refresh', 'read']);
  assert.equal(views.at(-1).status.state, 'integrity_fault', 'ACK never clears an incident');
  state = 'cleanup_complete';
  await ui.read();
  assert.equal(views.at(-1).status.state, 'cleanup_complete');
  ui.dispose();
});

test('explicit refresh accepts only the ACK then rereads actual status without claiming Known', async () => {
  const calls = [];
  const views = [];
  const ui = createTrustedTimeDiagnostics({
    async requestTrustedTimeRefresh() { calls.push('refresh'); return { protocolVersion: 1, state: 'accepted' }; },
    async inspectTrustedTime() { calls.push('read'); return status('cleanup_pending', true); },
  }, view => views.push(view));
  const one = ui.refresh();
  assert.equal(ui.refresh(), one);
  await one;
  assert.deepEqual(calls, ['refresh', 'read']);
  assert.equal(views.at(-1).accepted, true);
  assert.equal(views.at(-1).status.state, 'cleanup_pending');
  assert.equal(views.at(-1).status.refreshing, true);
  assert.equal(canRefreshTrustedTime(views.at(-1).status), false);
  ui.dispose();
});

test('cleanup_complete is diagnostic DATA; explicit refresh still waits for a new actual status', { timeout: 5000 }, async () => {
  const laterRead = deferred();
  const readStarted = deferred();
  const calls = [];
  const views = [];
  let reads = 0;
  const ui = createTrustedTimeDiagnostics({
    async inspectTrustedTime() {
      calls.push('read');
      reads += 1;
      if (reads === 1) return status('cleanup_complete');
      readStarted.resolve();
      return laterRead.promise;
    },
    async requestTrustedTimeRefresh() { calls.push('refresh'); return { protocolVersion: 1, state: 'accepted' }; },
  }, view => views.push(view));
  await ui.read();
  assert.equal(views.at(-1).status.state, 'cleanup_complete');
  assert.equal(canRefreshTrustedTime(views.at(-1).status), true);
  assert.deepEqual(calls, ['read'], 'A completed cleanup observation cannot auto-request time');
  const refresh = ui.refresh();
  assert.equal(ui.refresh(), refresh);
  await readStarted.promise;
  assert.deepEqual(calls, ['read', 'refresh', 'read']);
  assert.equal(views.at(-1).accepted, true);
  assert.equal(views.at(-1).status, null, 'The old cleanup result cannot become the post-refresh status');
  assert.equal(views.at(-1).busy, true);
  laterRead.resolve(status('source_unavailable'));
  await refresh;
  assert.equal(views.at(-1).status.state, 'source_unavailable');
  assert.equal(views.at(-1).busy, false);
  ui.dispose();
});

test('Busy, invalid ACK and failed reads do not fabricate host state or expose errors', async () => {
  for (const error of ['HOST_BUSY', new Error('private details must not be displayed')]) {
    const views = [];
    const ui = createTrustedTimeDiagnostics({
      async requestTrustedTimeRefresh() { throw error; },
      async inspectTrustedTime() { throw new Error('not requested'); },
    }, view => views.push(view));
    await ui.refresh();
    assert.equal(views.at(-1).status, null);
    assert.equal(views.at(-1).accepted, false);
    assert.equal(views.at(-1).error, error === 'HOST_BUSY' ? 'busy' : 'unavailable');
    assert.equal(views.at(-1).busy, false);
    ui.dispose();
  }
  let reads = 0;
  const views = [];
  const ui = createTrustedTimeDiagnostics({
    async requestTrustedTimeRefresh() { return { protocolVersion: 1, state: 'known' }; },
    async inspectTrustedTime() { reads += 1; return status(); },
  }, view => views.push(view));
  await ui.refresh();
  assert.equal(reads, 0);
  assert.equal(views.at(-1).error, 'invalidResponse');
  assert.equal(trustedTimeErrorCode('HOST_STATE_CORRUPT'), 'unavailable', 'only actual host status can label integrity_fault');
  ui.dispose();
  const afterAck = [];
  const readFailure = createTrustedTimeDiagnostics({
    async requestTrustedTimeRefresh() { return { protocolVersion: 1, state: 'accepted' }; },
    async inspectTrustedTime() { throw new Error('private status error'); },
  }, view => afterAck.push(view));
  await readFailure.refresh();
  assert.equal(afterAck.at(-1).accepted, true);
  assert.equal(afterAck.at(-1).status, null);
  assert.equal(afterAck.at(-1).error, 'unavailable');
  readFailure.dispose();
});

test('unmount and remount cannot publish late results or send a not-yet-started refresh', async () => {
  const pending = deferred();
  const oldViews = [];
  let reads = 0;
  const adapter = { inspectTrustedTime() { reads += 1; return pending.promise; } };
  const old = createTrustedTimeDiagnostics(adapter, view => oldViews.push(view));
  const request = old.read();
  await Promise.resolve();
  old.dispose();
  const count = oldViews.length;
  const freshViews = [];
  const fresh = createTrustedTimeDiagnostics({ async inspectTrustedTime() { return status('source_unavailable'); } }, view => freshViews.push(view));
  await fresh.read();
  pending.resolve(status('cleanup_pending'));
  await request;
  assert.equal(reads, 1);
  assert.equal(oldViews.length, count);
  assert.equal(freshViews.at(-1).status.state, 'source_unavailable');
  let sent = 0;
  const gone = createTrustedTimeDiagnostics({ requestTrustedTimeRefresh() { sent += 1; } }, () => null);
  const cancelledBeforeSend = gone.refresh();
  gone.dispose();
  await cancelledBeforeSend;
  assert.equal(sent, 0);
  fresh.dispose();
});

test('diagnostic capability is explicitly loaded and independent from legacy update', () => {
  assert.equal(can('trustedTimeDiagnostics'), false);
  assert.equal(can('trustedTimeDiagnostics', { trustedTimeDiagnostics: true }), false);
  assert.equal(can('trustedTimeDiagnostics', { loaded: true }), false);
  assert.equal(can('trustedTimeDiagnostics', { loaded: true, trustedTimeDiagnostics: true, appUpdate: false }), true);
  const source = readFileSync(new URL('../src/features/settings/SettingsView.jsx', import.meta.url), 'utf8');
  assert.match(source, /\(canUpdateApp \|\| canDiagnoseTime\) && <SectionButton id="update"/);
  assert.match(source, /\{canUpdateApp && \([\s\S]*?<IOSSection title=\{t\.uiSettings\.version\}>/);
  const platformSource = readFileSync(new URL('../src/shared/platform.js', import.meta.url), 'utf8');
  const desktop = { window: { PinvouPlatform: { kind: 'desktop', isWeb: false, capabilities: { appUpdate: false } } } };
  vm.runInNewContext(`${platformSource.replaceAll('export ', '')}\nglobalThis.legacyUpdate = can('appUpdate'); globalThis.diagnostics = can('trustedTimeDiagnostics', { loaded: true, trustedTimeDiagnostics: true });`, desktop);
  assert.equal(desktop.legacyUpdate, false);
  assert.equal(desktop.diagnostics, true, 'diagnostics must remain reachable without legacy update');
  const sandbox = { window: { PinvouPlatform: { kind: 'web', isWeb: true, capabilities: { trustedTimeDiagnostics: true } } } };
  vm.runInNewContext(`${platformSource.replaceAll('export ', '')}\nglobalThis.supported = can('trustedTimeDiagnostics', { loaded: true, trustedTimeDiagnostics: true });`, sandbox);
  assert.equal(sandbox.supported, false, 'web cannot borrow a desktop capability snapshot');
});

test('all three lazy dictionaries explain every host state, network action and accepted-only result', () => {
  for (const lang of ['zh', 'en', 'ja']) {
    const copy = dict[lang].trustedTime;
    const compare = (left, right) => left.localeCompare(right);
    assert.deepEqual(Object.keys(copy.states).toSorted(compare), [...TRUSTED_TIME_STATES].toSorted(compare));
    for (const text of [copy.title, copy.readStatus, copy.refresh, copy.networkNotice, copy.accepted,
      ...Object.values(copy.states), ...Object.values(copy.errors)]) {
      assert.equal(typeof text, 'string');
      assert.ok(text.trim());
    }
  }
});

test('the semantic bridge deduplicates only in-flight invokes and does not refresh during local reads', async () => {
  const source = readFileSync(new URL('../src/platform/tauri/bridge/updater.js', import.meta.url), 'utf8');
  const root = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  vm.runInNewContext(source, { window: root });
  const read = deferred();
  const refresh = deferred();
  const calls = [];
  const updater = root.__PINVOU_TAURI_BRIDGE_FEATURES__.updater({
    state: {}, notify() { return null; },
    invoke(command) {
      calls.push(command);
      return command === 'get_upgrade_trusted_time_status' ? read.promise : refresh.promise;
    },
  });
  const one = updater.inspectTrustedTime();
  assert.equal(updater.inspectTrustedTime(), one);
  await Promise.resolve();
  assert.deepEqual(calls, ['get_upgrade_trusted_time_status']);
  const two = updater.requestTrustedTimeRefresh();
  assert.equal(updater.requestTrustedTimeRefresh(), two);
  await Promise.resolve();
  assert.deepEqual(calls, ['get_upgrade_trusted_time_status', 'request_upgrade_trusted_time_refresh']);
  read.resolve(status());
  refresh.resolve({ protocolVersion: 1, state: 'accepted' });
  await Promise.all([one, two]);
  await updater.inspectTrustedTime();
  assert.equal(calls.length, 3, 'explicit later read is actual, not a cached status');
});


test('post-ACK status is a new observation and an older finalizer cannot evict it', async () => {
  const source = readFileSync(new URL('../src/platform/tauri/bridge/updater.js', import.meta.url), 'utf8');
  const root = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  vm.runInNewContext(source, { window: root });
  const oldRead = deferred();
  const newRead = deferred();
  const ack = deferred();
  let reads = 0;
  const updater = root.__PINVOU_TAURI_BRIDGE_FEATURES__.updater({
    state: {}, notify() {},
    invoke(command) {
      if (command === 'request_upgrade_trusted_time_refresh') return ack.promise;
      assert.equal(command, 'get_upgrade_trusted_time_status');
      return ++reads === 1 ? oldRead.promise : newRead.promise;
    },
  });
  const old = updater.inspectTrustedTime();
  await Promise.resolve();
  const refresh = updater.requestTrustedTimeRefresh();
  await Promise.resolve();
  ack.resolve({ protocolVersion: 1, state: 'accepted' });
  await refresh;
  const fresh = updater.inspectTrustedTime();
  assert.notEqual(fresh, old);
  await Promise.resolve();
  assert.equal(reads, 2, 'the post-ACK caller performs an actual new invoke');
  oldRead.resolve(status('unknown'));
  await old;
  assert.equal(updater.inspectTrustedTime(), fresh, 'old completion cannot clear the new in-flight read');
  newRead.resolve(status('cleanup_pending', true));
  assert.equal((await fresh).state, 'cleanup_pending');
});
