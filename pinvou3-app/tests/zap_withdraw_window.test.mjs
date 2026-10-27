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
  // The real chat.js machinery temporarily swaps the per-session working set
  // through runSyncOnSession for background-session drops; the stub below
  // mirrors that buffer switching so the harness exercises the same code
  // path (a no-op stub would hide a restore aimed at the wrong buffer).
  return (state, invoke, buffers = {}) => factory({
    state,
    sessionStates: buffers,
    invoke,
    notify() {},
    bt(key) { return key; },
    runSyncOnSession(sid, fn) {
      // Production (bridge.js runSyncOnSession) runs fn inline for a falsy
      // sid too; mirror that exactly.
      if (!sid || sid === state.activeSessionId) return fn();
      // Production returns early when the background buffer is missing (the
      // session was evicted); mirror that so the stub can never run fn
      // against the active working set under a foreign session id.
      if (!buffers[sid]) return;
      const original = { ...state };
      Object.assign(state, buffers[sid], { activeSessionId: sid });
      try { return fn(); } finally {
        Object.assign(buffers[sid], state);
        Object.assign(state, original);
      }
    },
    TAURI: true,
    turnUsageDirty: {},
    safeConsoleInfo() {},
    getBuffer(sid) { return buffers[sid] || null; },
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

test('a background-session drop restores its own draft and leaves the active session unchanged', async () => {
  const state = { activeSessionId: 'A', composerDraft: 'active draft', draftEpoch: 7,
    queued: [], busy: false, messages: [], chatItems: [], settings: { language: 'en' } };
  const buffer = { composerDraft: 'background draft', queued: [{ id: 'q1', text: 'recover me',
    displayText: 'recover me', steered: true, steerId: 'st-bg', attachments: [] }],
    busy: true, messages: [], chatItems: [] };
  let releaseWithdraw;
  const pending = new Promise((resolve) => { releaseWithdraw = resolve; });
  const calls = [];
  const api = loadChatApi()(state, (name) => {
    calls.push(name);
    return name === 'withdraw_steer' ? pending : Promise.resolve({});
  }, { B: buffer });
  const run = api.interruptAndSendQueued('B', 'q1');
  await waitForWithdrawInvoke(calls);
  try {
    api.settleSteerDropped('B', 'st-bg');
    releaseWithdraw('not_pending');
    assert.equal(await run, true);
    assert.equal(buffer.composerDraft, 'background draft\nrecover me');
    assert.equal(state.composerDraft, 'active draft');
    assert.equal(state.draftEpoch, 7);
    assert.equal(state.activeSessionId, 'A');
    assert.equal(state.chatItems.length, 0);
    assert.deepEqual([...notices(buffer)], ['⚠️ steerFailed']);
    // The resend goes out as invoke('chat', ...); 'send_message' is not a
    // real command on this path, so pin the live command name.
    assert.ok(!calls.includes('chat'), 'an uncertain withdrawal must never resend');
  } finally {
    api.clearOutcomeReconcileWatchdog('B', 'st-bg');
  }
});

test('the lost-message notice copy is non-empty and language-specific in every locale', () => {
  // The recovery notice is emitted by the bridge's own BT_TABLE before any
  // React dictionary is guaranteed to be loaded, so the copy must resolve
  // from that eager table alone: non-empty for every UI language, never the
  // bare key echoed back, and distinct per language (a copied fallback would
  // silently show one locale's sentence in the other two).
  const bridgeSource = fs.readFileSync(path.join(appRoot, 'src', 'platform', 'tauri', 'bridge.js'), 'utf8');
  const tableStart = bridgeSource.indexOf('const BT_TABLE = {');
  // The slice carries the shipped BT_TABLE plus the shipped bt forwarder;
  // the forwarder delegates through the shared-base cluster accessor.
  const sliceEnd = bridgeSource.indexOf('function isDefaultChatTitle', tableStart);
  assert.ok(tableStart >= 0 && sliceEnd > tableStart, 'BT_TABLE must stay declared in the desktop bridge');
  // bt delegates to the shared-base cluster accessor; the sandbox loads the
  // shared base and wires the accessor against the sliced BT_TABLE exactly
  // like the shipped bridge does.
  const btInSandbox = (language, key) => {
    const context = vm.createContext({ state: { settings: { language } }, console });
    vm.runInContext('this.window = this;', context);
    vm.runInContext(
      fs.readFileSync(path.join(appRoot, 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8'),
      context,
      { filename: 'src/shared/bridge-shared-helpers.js' },
    );
    vm.runInContext(`this.pinvouSharedtauriMain = () => this.__mainCluster || (this.__mainCluster = window.PinvouBridgeShared.create("tauriMain", {
      state, BT_TABLE: { get value() { return BT_TABLE; } }, invoke: async () => null,
      notify: () => {}, addChatItem: () => {},
    }));`, context);
    vm.runInContext(bridgeSource.slice(tableStart, sliceEnd), context, {
      filename: 'src/platform/tauri/bridge.js#BT_TABLE',
    });
    return vm.runInContext(`bt(${JSON.stringify(key)})`, context);
  };
  const perLanguage = ['en', 'ja', 'zh'].map((language) => btInSandbox(language, 'steerFailedLost'));
  for (const [index, language] of ['en', 'ja', 'zh', 'zh-Hans'].entries()) {
    const resolved = language === 'zh-Hans' ? btInSandbox(language, 'steerFailedLost') : perLanguage[index];
    assert.equal(typeof resolved, 'string', `${language}: the notice must resolve to a string`);
    assert.ok(resolved.length > 0, `${language}: the notice must not be empty`);
    assert.notEqual(resolved, 'steerFailedLost', `${language}: the key must not echo back untranslated`);
    assert.match(resolved, /^(?!⚠️)/, `${language}: the prefix is added by the caller, the copy stays bare`);
  }
  assert.equal(new Set(perLanguage).size, 3, 'en/ja/zh copies must be pairwise distinct');
});
