/**
 * Zap withdraw-window recovery (chat:steer_dropped mid-withdraw): driving the
 * real chat.js factory, a chat:steer_dropped landing while runQueuedZap awaits
 * withdraw_steer must recover the message instead of silently consuming the
 * withdrawn registration. The notice keys on the restore verdict: a restored
 * text reads the "restored to the input" variant; only a text that restores
 * nothing (attachment-only chip) reads the lost variant.
 */
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(here, '..');

function loadChatApi() {
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
  vm.runInContext(
    fs.readFileSync(path.join(appRoot, 'src', 'platform', 'tauri', 'bridge', 'chat.js'), 'utf8'),
    context,
    { filename: 'src/platform/tauri/bridge/chat.js' },
  );
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;
  return (state, invoke) => factory({
    state,
    sessionStates: {},
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
}

function notices(state) {
  return state.chatItems.filter((i) => i && i.type === 'system').map((i) => i.text);
}

async function waitForWithdrawInvoke(invokeCalls) {
  // Bounded wait: a regression that prevents the withdraw invoke must fail
  // fast instead of hanging the whole suite (node.test has no per-test
  // default timeout).
  for (let waited = 0; waited < 5000 && !invokeCalls.includes('withdraw_steer'); waited += 10) {
    await new Promise((r) => { setTimeout(r, 10); });
  }
  assert.ok(invokeCalls.includes('withdraw_steer'), 'the withdraw invoke must have started');
}

test('a dropped inside the zap withdraw window restores the text and says so', async () => {
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
  let releaseWithdraw;
  const withdrawPending = new Promise((resolve) => {
    releaseWithdraw = resolve;
  });
  const invokeCalls = [];
  const api = loadChatApi()(state, (name) => {
    invokeCalls.push(name);
    if (name === 'withdraw_steer') return withdrawPending;
    return Promise.resolve({});
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
  await waitForWithdrawInvoke(invokeCalls);
  try {
    // The dropped event lands mid-withdraw: the zap-owned claim must route it
    // to the recovery (failure notice + composer restore) instead of the
    // silent consume that lost the message.
    api.settleSteerDropped('A', 'st-9');
    releaseWithdraw('not_pending');
    const verdict = await run;

    assert.equal(verdict, true, 'the zap itself reports handled (skip-resend)');
    assert.deepEqual(state.queued, [], 'the proven-dropped chip must not be re-queued');
    assert.equal(state.composerDraft, '总结一下当前进度', 'the message text is restored to the composer');
    const noticeTexts = notices(state);
    assert.ok(
      noticeTexts.includes('⚠️ steerFailed'),
      `the restored variant must fire: ${JSON.stringify(noticeTexts)}`,
    );
    assert.ok(
      !noticeTexts.includes('⚠️ steerFailedLost'),
      `a restored text must not claim it could not be restored: ${JSON.stringify(noticeTexts)}`,
    );
    // A late duplicate dropped after the recovery consumed the registration
    // and the claim must stay silent (exactly-once, no false second notice).
    api.settleSteerDropped('A', 'st-9');
    assert.equal(
      notices(state).filter((t) => t === '⚠️ steerFailed' || t === '⚠️ steerFailedLost').length,
      1,
      'the duplicate dropped event must not fire a second recovery notice',
    );
  } finally {
    // A failed assertion must not leave a real 60s watchdog timer stretching
    // the red run.
    api.clearOutcomeReconcileWatchdog('A', 'st-9');
  }
});

test('a dropped inside the window with nothing restorable reports the lost variant', async () => {
  // An attachment-only chip has no text: the recovery can queue nothing back
  // to the composer, and the notice must say the message could not be
  // restored instead of claiming a restoration that did not happen.
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
  let releaseWithdraw;
  const withdrawPending = new Promise((resolve) => {
    releaseWithdraw = resolve;
  });
  const invokeCalls = [];
  const api = loadChatApi()(state, (name) => {
    invokeCalls.push(name);
    if (name === 'withdraw_steer') return withdrawPending;
    return Promise.resolve({});
  });

  const item = {
    id: 'q1',
    text: '',
    payloadText: null,
    displayText: '',
    steered: true,
    steerId: 'st-9',
    attachments: [{ name: 'spec.pdf', path: '/tmp/spec.pdf', size: 1024 }],
  };
  state.queued = [item];

  const run = api.interruptAndSendQueued('A', 'q1');
  await waitForWithdrawInvoke(invokeCalls);
  try {
    api.settleSteerDropped('A', 'st-9');
    releaseWithdraw('not_pending');
    const verdict = await run;

    assert.equal(verdict, true, 'the zap itself reports handled (skip-resend)');
    assert.deepEqual(state.queued, [], 'the proven-dropped chip must not be re-queued');
    assert.equal(state.composerDraft, '', 'nothing is restorable for an attachment-only chip');
    const noticeTexts = notices(state);
    assert.ok(
      noticeTexts.includes('⚠️ steerFailedLost'),
      `the lost variant must fire: ${JSON.stringify(noticeTexts)}`,
    );
    assert.ok(
      !noticeTexts.includes('⚠️ steerFailed'),
      `no restoration happened, the restored variant would lie: ${JSON.stringify(noticeTexts)}`,
    );
  } finally {
    api.clearOutcomeReconcileWatchdog('A', 'st-9');
  }
});
