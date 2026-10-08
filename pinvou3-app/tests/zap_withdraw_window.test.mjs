/**
 * Zap withdraw-window recovery (steer_dropped mid-withdraw): driving the real
 * chat.js factory, a chat:steer_dropped landing while runQueuedZap awaits
 * withdraw_steer must recover the message (failure notice + composer restore)
 * instead of silently consuming the withdrawn registration.
 */
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(here, '..');


test('a steer_dropped inside the zap withdraw window recovers instead of going silent', async () => {
  const state = {
    activeSessionId: 'A',
    composerDraft: '',
    draftEpoch: 0,
    queued: [],
    busy: true,
    messages: [],
    chatItems: [],
    settings: { language: 'en' },
  };
  const sessionStates = {};
  const root = {};
  const context = vm.createContext({
    window: root,
    globalThis: root,
    setTimeout,
    clearTimeout,
    console,
  });
  vm.runInContext(
    fs.readFileSync(path.join(appRoot, 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8'),
    context,
    { filename: 'src/shared/bridge-shared-helpers.js' },
  );
  const registrySrc = fs.readFileSync(
    path.join(appRoot, 'src', 'platform', 'tauri', 'bridge', 'chat.js'),
    'utf8',
  );
  vm.runInContext(registrySrc, context, { filename: 'src/platform/tauri/bridge/chat.js' });

  let releaseWithdraw;
  const withdrawPending = new Promise((resolve) => {
    releaseWithdraw = resolve;
  });
  const invokeCalls = [];
  const invoke = (name) => {
    invokeCalls.push(name);
    if (name === 'withdraw_steer') return withdrawPending;
    return Promise.resolve({});
  };

  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;
  const api = factory({
    state,
    sessionStates,
    invoke,
    notify() {},
    bt(key) { return key; },
    runSyncOnSession(sid, fn) { fn(); },
    TAURI: true,
    turnUsageDirty: {},
    safeConsoleInfo() {},
    getBuffer() { return null; },
    timeStr() { return ''; },
  });

  const item = {
    id: 'q1',
    text: '总结一下当前进度',
    payloadText: null,
    displayText: '总结一下当前进度',
    steered: true,
    steerId: 'st-9',
    attachments: [],
  };
  state.queued = [item];

  const run = api.interruptAndSendQueued('A', 'q1');
  while (!invokeCalls.includes('withdraw_steer')) {
    await new Promise((r) => { setTimeout(r, 1); });
  }
  // The dropped event lands mid-withdraw: the zap-owned claim must route it
  // to the recovery (failure notice + composer restore) instead of the
  // silent consume that lost the message.
  api.settleSteerDropped('A', 'st-9');
  releaseWithdraw('not_pending');
  const verdict = await run;

  assert.equal(verdict, true, 'the zap itself reports handled (skip-resend)');
  assert.deepEqual(state.queued, [], 'the proven-dropped chip must not be re-queued');
  assert.equal(state.composerDraft, '总结一下当前进度', 'the message text is restored to the composer');
  const noticeTexts = state.chatItems.filter((i) => i && i.type === 'system').map((i) => i.text);
  assert.ok(
    noticeTexts.includes('⚠️ steerFailedLost'),
    `the lost variant must fire: ${JSON.stringify(noticeTexts)}`,
  );
  assert.equal(
    noticeTexts.filter((t) => t.includes('steerFailedLost')).length,
    1,
    'exactly one recovery notice — no duplicate from the watchdog expiry',
  );
  // The reconcile watchdog was armed by settleZapSkipResend; its expiry is
  // silent now (the registration was consumed by the recovery). Clear it so
  // the test process does not wait out the window.
  api.clearOutcomeReconcileWatchdog('A', 'st-9');
});
