/**
 * Session-mention restore / steer-loss behavioral tests (PR #586 review
 * round-8 M1/M2/M3).
 *
 * Loaded with the REAL bridge-shared-helpers.js and the REAL tauri chat.js
 * feature factory in a vm context, stubbing only the lane-level deps:
 * - the onSteerFailure branch table must route the failure notice on the
 *   restoreSteerText verdict: a refs-only message strips to "" (nothing is
 *   restored), so the chip degrades to a plain queued entry and the
 *   steerFailedQueued variant is emitted — never the "restored to the input"
 *   claim;
 * - queuedPayloadEnvelope must refuse payloads that do not carry the
 *   injection block at their head (guide-prefixed scene payloads used to be
 *   sliced mid-JSON) and must anchor on the trimmed body (a stored body with
 *   interior leading whitespace used to make the queued edit impossible);
 * - stripMentionBlockForComposerRestore strips the machine block on restore.
 */
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { splitSessionMentionBlock, buildSessionMentionBlock } from '../src/features/chat/session-mention.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(here, '..');

const BT = {
  steerFailed: 'steerFailed',
  steerFailedQueued: 'steerFailedQueued',
};

function makeVmRoot() {
  const root = {
    __PINVOU_SESSION_MENTION__: { splitSessionMentionBlock },
  };
  const context = vm.createContext({
    window: root,
    globalThis: root,
    setTimeout,
    clearTimeout,
    console,
  });
  vm.runInContext(
    readNormalized('src', 'shared', 'bridge-shared-helpers.js'),
    context,
    { filename: 'src/shared/bridge-shared-helpers.js' },
  );
  return root;
}

// Windows-CRLF portability (round-20 zhuowp): normalize source reads so
// loader markers and brace matching behave identically on CRLF checkouts.
const readNormalized = (...segments) => fs.readFileSync(path.join(appRoot, ...segments), 'utf8').replace(/\r\n/g, '\n');

function loadChatFeature(root) {
  const context = vm.createContext({
    window: root,
    globalThis: root,
    setTimeout,
    clearTimeout,
    console,
  });
  vm.runInContext(
    readNormalized('src', 'shared', 'bridge-shared-helpers.js'),
    context,
    { filename: 'src/shared/bridge-shared-helpers.js' },
  );
  vm.runInContext(
    readNormalized('src', 'platform', 'tauri', 'bridge', 'chat.js'),
    context,
    { filename: 'src/platform/tauri/bridge/chat.js' },
  );
  return root.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;
}

function makeHarness({ ensureSession, invoke } = {}) {
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
  const root = makeVmRoot();
  const factory = loadChatFeature(root);
  const invokeImpl = invoke || (() => Promise.resolve({}));
  const api = factory({
    state,
    sessionStates,
    invoke: invokeImpl,
    ensureSession,
    notify() {},
    bt(key) { return BT[key] === undefined ? key : BT[key]; },
    runSyncOnSession(sid, fn) { fn(); },
    TAURI: true,
    turnUsageDirty: {},
    safeConsoleInfo() {},
    getBuffer() { return null; },
    timeStr() { return ''; },
  });
  // System notices land in the chat slice via the shared cluster's own
  // addChatItem — read them from the state, not from injected stubs.
  const notices = () => state.chatItems.filter(i => i && i.type === 'system').map(i => i.text);
  const shared = root.PinvouBridgeShared.create('tauriChat', {
    state,
    sessionStates,
    invoke: () => Promise.resolve({}),
    notify() {},
    bt(key) { return BT[key] === undefined ? key : BT[key]; },
    messageHasToolBlock() { return false; },
    summonPinvou() {},
  });
  return { state, sessionStates, notices, shared, api };
}

const REFS = [{ sessionId: 'abc123', title: '设计稿' }];
const REFS_ONLY_BLOCK = buildSessionMentionBlock(REFS);
const BLOCK_AND_BODY = REFS_ONLY_BLOCK + '把配色用到 PPT 里';

function steerItem(text) {
  return { id: 'q1', text, payloadText: null, displayText: text, steered: true, steerId: 'st-1', attachments: [] };
}

test('onSteerFailure: refs-only steer failure keeps the chip queued and emits the queued-variant notice', () => {
  const { state, notices, api } = makeHarness();
  const item = steerItem(REFS_ONLY_BLOCK);
  state.queued = [item];
  state.composerDraft = '';
  api.onSteerFailure(item, 'A', { snapshot: null }, REFS_ONLY_BLOCK, new Error('boom'));
  // Restore-first: a refs-only message restores nothing, so the chip must
  // stay in the queue, degraded to a plain entry flushQueued can re-send.
  assert.equal(state.queued.length, 1, 'the chip must not be spliced on a refs-only failure');
  assert.equal(item.steered, false);
  assert.equal(item.steerId, null);
  assert.equal(state.composerDraft, '', 'nothing may be handed back for a refs-only message');
  assert.ok(notices().includes('⚠️ steerFailedQueued'), `got: ${JSON.stringify(notices())}`);
  assert.ok(!notices().includes('⚠️ steerFailed'), 'the restored-variant notice must not fire');
});

test('onSteerFailure: body steer failure with an empty composer restores the stripped body and splices the chip', () => {
  const { state, notices, api } = makeHarness();
  const item = steerItem(BLOCK_AND_BODY);
  state.queued = [item];
  state.composerDraft = '';
  api.onSteerFailure(item, 'A', { snapshot: null }, BLOCK_AND_BODY, new Error('boom'));
  assert.equal(state.queued.length, 0, 'a restored steer splices its chip');
  assert.equal(state.composerDraft, '把配色用到 PPT 里', 'the injection block must not re-enter the composer');
  assert.ok(notices().includes('⚠️ steerFailed'), `got: ${JSON.stringify(notices())}`);
});

test('onSteerFailure: occupied composer degrades the chip and emits the queued-variant notice', () => {
  const { state, notices, api } = makeHarness();
  const item = steerItem(BLOCK_AND_BODY);
  state.queued = [item];
  state.composerDraft = '用户正在输入';
  api.onSteerFailure(item, 'A', { snapshot: null }, BLOCK_AND_BODY, new Error('boom'));
  assert.equal(state.queued.length, 1);
  assert.equal(item.steered, false);
  assert.equal(state.composerDraft, '用户正在输入', 'an occupied composer must not be clobbered');
  assert.ok(notices().includes('⚠️ steerFailedQueued'), `got: ${JSON.stringify(notices())}`);
});

test('onSteerFailure: a chip taken over by ×/zap (no longer queued) stays silent', () => {
  const { state, notices, api } = makeHarness();
  const item = steerItem(BLOCK_AND_BODY);
  state.queued = [];
  state.composerDraft = '';
  api.onSteerFailure(item, 'A', { snapshot: null }, BLOCK_AND_BODY, new Error('boom'));
  assert.deepEqual(notices(), []);
  assert.equal(state.composerDraft, '', 'a taken-over chip owns its own recovery');
});

test('onSteerFailure: session-switched chip degrades in its own queue without touching the active draft', () => {
  const { state, sessionStates, notices, api } = makeHarness();
  state.activeSessionId = 'B';
  state.composerDraft = '当前会话草稿';
  // busy keeps onSteerFailure's idle flush from actually sending the
  // degraded chip — this test pins the queue state, not the send machinery.
  sessionStates.A = { composerDraft: '', queued: [], busy: true };
  const item = steerItem(BLOCK_AND_BODY);
  sessionStates.A.queued = [item];
  api.onSteerFailure(item, 'A', { snapshot: null }, BLOCK_AND_BODY, new Error('boom'));
  // Switched away: the chip degrades in session A's queue (flushQueued
  // re-sends it there at turn end); neither the active draft nor A's draft
  // receives the text, and the queued-variant notice fires.
  assert.equal(sessionStates.A.queued.length, 1);
  assert.equal(item.steered, false);
  assert.equal(sessionStates.A.composerDraft, '');
  assert.equal(state.composerDraft, '当前会话草稿');
  assert.ok(notices().includes('⚠️ steerFailedQueued'), `got: ${JSON.stringify(notices())}`);
});

test('settleSteerDropped: refs-only engine drop degrades to a plain queued entry instead of losing the message (round-9)', () => {
  // The engine dropped the steer: for a refs-only message the restore strips
  // to "" and hands nothing back. The chip used to be spliced BEFORE the
  // restore verdict — the message vanished entirely (chip gone, nothing
  // restored, composer chips already consumed at dispatch). The round-6 M1
  // restore-first pattern applies here too: the chip stays queued as a plain
  // entry flushQueued re-sends, with the queued-variant notice.
  const { state, notices, api } = makeHarness();
  const item = steerItem(REFS_ONLY_BLOCK);
  state.queued = [item];
  state.composerDraft = '';
  api.settleSteerDropped('A', 'st-1');
  assert.equal(state.queued.length, 1, 'a refs-only drop must keep the chip queued');
  assert.equal(item.steered, false);
  assert.equal(item.steerId, null);
  assert.equal(state.composerDraft, '', 'nothing restorable for a refs-only message');
  assert.ok(notices().includes('⚠️ steerDroppedQueued'), `got: ${JSON.stringify(notices())}`);
  assert.ok(!notices().includes('⚠️ steerDropped'), 'the cancelled-variant notice must not fire');
});

test('settleSteerDropped: body engine drop restores the stripped body and splices the chip', () => {
  const { state, notices, api } = makeHarness();
  const item = steerItem(BLOCK_AND_BODY);
  state.queued = [item];
  state.composerDraft = '';
  api.settleSteerDropped('A', 'st-1');
  assert.equal(state.queued.length, 0, 'a restored drop splices its chip');
  assert.equal(state.composerDraft, '把配色用到 PPT 里', 'the body comes back without the block');
  assert.ok(notices().includes('⚠️ steerDropped'), `got: ${JSON.stringify(notices())}`);
});

test('queuedPayloadEnvelope: guide-prefixed scene payload refuses the block-aware split instead of slicing mid-JSON', () => {
  const { shared } = makeHarness();
  const guide = '下面是创建定时任务的说明，请按格式回复：\n\n';
  const template = 'PROMPT: 总结\n用户需求：\n把配色用到 PPT 里\n请生成';
  const payload = guide + REFS_ONLY_BLOCK + template;
  const user = BLOCK_AND_BODY;
  const envelope = shared.queuedPayloadEnvelope(user, payload, null);
  // The legacy refusal (null) is the safe outcome: the block does not sit at
  // the payload head, so slicing blockText.length off would cut into the
  // original block's JSON (round-8 M2 corruption shape).
  assert.equal(envelope, null, `got: ${JSON.stringify(envelope)}`);
});

test('queuedPayloadEnvelope: block-at-head scene payload still produces the block-aware envelope', () => {
  const { shared } = makeHarness();
  const body = '把配色用到 PPT 里';
  const scaffold = 'PROMPT: 总结\n用户需求：\n';
  const payload = REFS_ONLY_BLOCK + scaffold + body + '\n请生成';
  const envelope = shared.queuedPayloadEnvelope(BLOCK_AND_BODY, payload, null);
  assert.ok(envelope && envelope.blockAware, `got: ${JSON.stringify(envelope)}`);
  assert.equal(envelope.blockPrefix, REFS_ONLY_BLOCK);
  assert.equal(envelope.before, scaffold);
  assert.equal(envelope.after, '\n请生成');
});

test('queuedPayloadEnvelope: interior leading whitespace in the stored body anchors on the trimmed body', () => {
  const { shared } = makeHarness();
  const user = REFS_ONLY_BLOCK + '  带前导空格的正文';
  const scaffold = 'PROMPT: 总结\n用户需求：\n';
  const payload = REFS_ONLY_BLOCK + scaffold + '带前导空格的正文' + '\ntail';
  const envelope = shared.queuedPayloadEnvelope(user, payload, null);
  assert.ok(envelope && envelope.blockAware, `the trimmed body must match: ${JSON.stringify(envelope)}`);
  assert.equal(envelope.before, scaffold);
  const rebuilt = shared.rebuiltQueuedPayload(
    { payloadEnvelope: envelope },
    buildSessionMentionBlock([{ sessionId: 'xyz789', title: '新引用' }]) + '编辑后的正文',
  );
  assert.equal(rebuilt, buildSessionMentionBlock([{ sessionId: 'xyz789', title: '新引用' }]) + scaffold + '编辑后的正文' + '\ntail');
});

test('queuedPayloadEnvelope: refs-only queued text stays on the legacy exact-match path (no blockAware)', () => {
  const { shared } = makeHarness();
  const payload = REFS_ONLY_BLOCK + 'scaffold-without-body';
  const envelope = shared.queuedPayloadEnvelope(REFS_ONLY_BLOCK, payload, null);
  // The legacy indexOf path splits at the block head; the block-aware branch
  // requires a body anchor and must not fire.
  assert.ok(envelope && !envelope.blockAware, `got: ${JSON.stringify(envelope)}`);
  assert.equal(envelope.before, '');
  assert.equal(envelope.after, 'scaffold-without-body');
});

test('stripMentionBlockForComposerRestore: strips the block, keeps the body, empties refs-only text', () => {
  const { shared } = makeHarness();
  assert.equal(shared.stripMentionBlockForComposerRestore(BLOCK_AND_BODY), '把配色用到 PPT 里');
  assert.equal(shared.stripMentionBlockForComposerRestore(REFS_ONLY_BLOCK), '');
  assert.equal(shared.stripMentionBlockForComposerRestore('纯文本'), '纯文本');
});

test('materialization abort resolves "restored" for a refs-only message (round-18 MAJOR-1)', async () => {
  // The abort branch: the user navigated to an existing session while
  // create_session was in flight, so ensureSession's materialization does
  // not own the active scope. A refs-only message has nothing restorable —
  // it must STILL resolve "restored" (not false): the caller's failure tail
  // keys the provisional-mapping clear on this verdict, and a false here
  // arms the draft's chips live in the unrelated session (the round-18
  // defect). Drives the real sendMessage against the real shared helpers.
  const { state, api } = makeHarness({
    // Mid-create navigation: the user clicks an existing session while
    // create_session is in flight, so the materialization does not own the
    // active scope when it lands.
    ensureSession: async () => {
      state.activeSessionId = 'OTHER';
      return 'CREATED-BUT-NOT-ACTIVE';
    },
  });
  state.attachments = [];
  state.activeSessionId = null;
  state.draftEpoch = 7;
  const verdict = await api.sendMessage(buildSessionMentionBlock(REFS));
  assert.equal(verdict, 'restored', 'a refs-only aborted materialization resolves restored');
  assert.equal(state.composerDraft, '', 'nothing is re-prefilled for a refs-only abort');
  // A textful abort keeps the restore-and-verdict shape (round-8 M1's
  // surviving half): the stripped body goes back under the draft ownership —
  // but only when the restore actually LANDS (round-23 asto minor 1): with
  // the created session's background buffer live, the text is written there
  // and the verdict is "restored".
  const { state: textState, sessionStates: textStates, api: textApi } = makeHarness({
    ensureSession: async () => {
      textState.activeSessionId = 'OTHER';
      return 'CREATED-BUT-NOT-ACTIVE';
    },
  });
  textState.attachments = [];
  textState.activeSessionId = null;
  textState.draftEpoch = 8;
  textStates['CREATED-BUT-NOT-ACTIVE'] = { composerDraft: '' };
  const textful = await textApi.sendMessage(buildSessionMentionBlock(REFS) + '把配色用到 PPT 里');
  assert.equal(textful, 'restored', 'a textful aborted materialization with a live buffer restores and resolves restored');
  assert.equal(
    textStates['CREATED-BUT-NOT-ACTIVE'].composerDraft,
    '把配色用到 PPT 里',
    'the stripped body landed in the created session\'s buffer',
  );
  // The evicted/purged-buffer window (round-23 asto minor 1): the created
  // session's background buffer is gone (LRU eviction across 32+ switches, or
  // a purge inside the create-await window) — restoreSteerText lands NOWHERE,
  // so restoreTaskDraft must report the miss: a phantom "restored" made the
  // caller skip its own recovery and the text was silently gone.
  const { state: goneState, api: goneApi } = makeHarness({
    ensureSession: async () => {
      goneState.activeSessionId = 'OTHER';
      return 'CREATED-BUT-GONE';
    },
  });
  goneState.attachments = [];
  goneState.activeSessionId = null;
  goneState.draftEpoch = 9;
  const gone = await goneApi.sendMessage(buildSessionMentionBlock(REFS) + '把配色用到 PPT 里');
  assert.equal(gone, false, 'an aborted materialization whose restore landed nowhere resolves false (the caller owns the recovery)');
});

// Round-24 MAJOR, bridge leg: a COMMITTED materialization reports the new
// session id through the caller-owned meta object — the send closure's
// unmount-proof ledger recording keys on it (the abort branch above must NOT
// set it). Drives the real sendMessage against the real shared helpers.
test('a committed draft materialization reports the session id on the caller-owned meta (round-24 MAJOR)', async () => {
  const { state, api } = makeHarness({
    ensureSession: async () => {
      state.activeSessionId = 'NEW';
      return 'NEW';
    },
  });
  state.attachments = [];
  state.activeSessionId = null;
  state.draftEpoch = 3;
  const meta = {};
  // Whatever the send itself does afterwards (verdict or throw), the report
  // is written at the materialization, before any downstream failure.
  await api.sendMessage('正文', meta).catch(() => {});
  assert.equal(meta.materializedSessionId, 'NEW', 'the committed materialization is reported through the meta out-param');
  // The abort branch never sets it: no phantom mapping for a draft that is
  // still alive.
  const { state: abortState, api: abortApi } = makeHarness({
    ensureSession: async () => {
      abortState.activeSessionId = 'OTHER';
      return 'CREATED-BUT-NOT-ACTIVE';
    },
  });
  abortState.attachments = [];
  abortState.activeSessionId = null;
  abortState.draftEpoch = 4;
  const abortMeta = {};
  await abortApi.sendMessage('正文', abortMeta).catch(() => {});
  assert.equal(abortMeta.materializedSessionId, undefined, 'an aborted materialization reports nothing');
});

// The zap withdraw-window recovery test (round-10 M2) lives in
// tests/zap_withdraw_window.test.mjs, which arrived on main through #675:
// the underlying zapReconcileClaims mechanism is mention-independent and
// was separated from this branch per review rounds 10-12. The
// mention-specific shape it covered here (a refs-only message restoring
// nothing → the lost variant) stays covered there through the
// attachment-only chip case.

// Round-25 asto minor 5: the 25s-timeout rejection is a distinct failure mode
// (engine alive-but-wedged) — the notice must not blame "session unavailable
// or engine not running" there, and the timed-out variants exist in all three
// locales (the count-3 guard in session_mention.test.mjs pins the BT table).
test('onSteerFailure: the 25s timeout routes the timed-out notice variants instead of the unavailable-engine cause', () => {
  const { state, notices, api } = makeHarness();
  const restored = steerItem(BLOCK_AND_BODY);
  state.queued = [restored];
  state.composerDraft = '';
  api.onSteerFailure(restored, 'A', { snapshot: null }, BLOCK_AND_BODY, new Error('steer_chat timed out'));
  assert.equal(state.queued.length, 0, 'the restore itself is unaffected by the cause');
  assert.ok(notices().includes('⚠️ steerFailedTimedOut'), `got: ${JSON.stringify(notices())}`);
  const occupied = steerItem(BLOCK_AND_BODY);
  state.queued = [occupied];
  state.composerDraft = '用户正在输入';
  api.onSteerFailure(occupied, 'A', { snapshot: null }, BLOCK_AND_BODY, new Error('steer_chat timed out'));
  assert.ok(notices().includes('⚠️ steerFailedQueuedTimedOut'), `got: ${JSON.stringify(notices())}`);
  // The generic cause keeps its own variants on non-timeout rejections.
  const plain = steerItem(BLOCK_AND_BODY);
  state.queued = [plain];
  state.composerDraft = '';
  api.onSteerFailure(plain, 'A', { snapshot: null }, BLOCK_AND_BODY, new Error('engine not running'));
  assert.ok(notices().includes('⚠️ steerFailed'), `got: ${JSON.stringify(notices())}`);
});

// Round-26 asto m2(a): ensureSession's commit-then-navigate tail commits the
// session to a background buffer and returns null — the materialization is
// REAL (the draft epoch is dead), so the meta report must fire and the
// round-25 "restored-materialized" mapping can route the chips. The rollback
// tail (draft re-entered with a fresh epoch) reports nothing.
test('commit-then-navigate reports the created session through the meta out-param; the rollback tail does not (round-26 asto m2a)', async () => {
  const invocations = [];
  const { state, sessionStates, api } = makeHarness({
    ensureSession: async (owner) => {
      state.activeSessionId = 'OTHER';
      owner.createdSessionId = 'BG-CREATED';
      return null;
    },
    invoke: (cmd) => { invocations.push(String(cmd)); return Promise.resolve({}); },
  });
  state.attachments = [];
  state.activeSessionId = null;
  state.draftEpoch = 11;
  sessionStates['BG-CREATED'] = { composerDraft: '' };
  const meta = {};
  const verdict = await api.sendMessage(buildSessionMentionBlock(REFS) + '把配色用到 PPT 里', meta);
  assert.equal(meta.materializedSessionId, 'BG-CREATED', 'the commit-then-navigate tail reports the committed background session');
  assert.equal(verdict, 'restored', 'the textful restore lands in the created session buffer (the caller then maps restored-materialized)');
  assert.equal(sessionStates['BG-CREATED'].composerDraft, '把配色用到 PPT 里');

  const { state: rbState, api: rbApi } = makeHarness({
    ensureSession: async (owner) => {
      rbState.activeSessionId = null;
      owner.createdSessionId = 'ROLLED-BACK';
      owner.rollbackFromDraftEpoch = 11;
      owner.draftEpoch = 12;
      return null;
    },
  });
  rbState.attachments = [];
  rbState.activeSessionId = null;
  rbState.draftEpoch = 11;
  const rbMeta = {};
  await rbApi.sendMessage(buildSessionMentionBlock(REFS) + '把配色用到 PPT 里', rbMeta).catch(() => {});
  assert.equal(rbMeta.materializedSessionId, undefined, 'the rollback tail keeps the draft alive and reports nothing');
});

// Round-26 asto m3: on the 25s-timeout verdict the degraded queued chip must
// NOT auto-resend — flushQueued is suppressed until the truth is known; the
// ordinary rejection keeps the turn-end compensation.
test('onSteerFailure: the timeout verdict suppresses the compensating auto-resend flush (round-26 asto m3)', async () => {
  const makeCounting = () => {
    const invocations = [];
    return {
      invocations,
      harness: makeHarness({
        invoke: (cmd) => { invocations.push(String(cmd)); return Promise.resolve({}); },
      }),
    };
  };
  // refs-only chip, empty composer: the chip degrades to a queued local entry
  // — exactly the auto-resend candidate the gate must hold back.
  const timedOut = makeCounting();
  timedOut.harness.state.queued = [steerItem(REFS_ONLY_BLOCK)];
  timedOut.harness.state.composerDraft = '';
  timedOut.harness.state.busy = false;
  timedOut.harness.api.onSteerFailure(steerItem(REFS_ONLY_BLOCK), 'A', { snapshot: null }, REFS_ONLY_BLOCK, new Error('steer_chat timed out'));
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.equal(
    timedOut.invocations.filter((cmd) => cmd === 'chat').length,
    0,
    'the timeout verdict must not auto-send the degraded chip',
  );
  // The non-timeout path keeps its compensation flush (the harness's null
  // buffer prevents the send itself from reaching invoke, so the gate line
  // is pinned at the source instead).
  const chatSource = readNormalized('src', 'platform', 'tauri', 'bridge', 'chat.js');
  assert.match(
    chatSource,
    /if \(!steerTimedOut && !isBusyFor\(sid\)\) flushQueued\(sid\);/,
    'the compensating flush stays gated on the timeout verdict',
  );
});
