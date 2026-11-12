#!/usr/bin/env node
// r6 审 m4：r5 修复波的两个 JS 行为此前零测试——(1) 召唤序号计数器在 sidecar
// 载入时按持久 seq 抬底（重启清零否则 Rust max_by_key(seq) 把旧 pass 当最近
// 一轮，每次点击都全量重审，r5 审 M1）；(2) 账目落盘串行链（并发 settle 的
// 全量快照乱序覆盖会永久丢 entry，r5 审 m6）。与 pinvou_scene_sidecar.test.js
// 同款 vm 抽取范式：从 bridge-shared-helpers.js 源码里抽出真实函数体执行，
// 不 mock 行为本身。删掉抬底循环 / 拆掉串行链，本文件必须红。
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { test } = require('node:test');
const assert = require('node:assert');

const sharedBaseSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'shared', 'bridge-shared-helpers.js'),
  'utf8',
);

function extractLoaderBlock(source) {
  const match = source.match(
    /async function loadPinvouReviewsForSession\(sid\) \{[\s\S]*?return \[\];\n *\}\n *\}\n/,
  );
  assert.ok(match, 'loadPinvouReviewsForSession 函数体未找到——抽取正则与源码漂移');
  return match[0];
}

function extractSaveChainBlock(source) {
  const match = source.match(
    /let pinvouSaveChain = Promise\.resolve\(\);[\s\S]*?function persistPinvouReviews\(\) \{[\s\S]*?return pinvouSaveChain;\n *\}/,
  );
  assert.ok(match, 'pinvouSaveChain/persistPinvouReviews 块未找到——抽取正则与源码漂移');
  return match[0];
}

function extractSummonBlock(source) {
  const match = source.match(
    /async function summonPinvou\(focus, mode\) \{[\s\S]*?notify\(\);\n *\}\n *\}\n/,
  );
  assert.ok(match, 'summonPinvou 函数体未找到——抽取正则与源码漂移');
  return match[0];
}

// r9 审 m7：restore 路径解守卫（r5 修复）此前零自动化覆盖——删
// `state.pinvouSummoning = false` 那行全绿。抽取真实 loadWorkingSetFrom
// 行为级复放：守卫被解、弹窗被关、数组按 buffer rebind。
function extractRestoreBlock(source) {
  const match = source.match(
    /function loadWorkingSetFrom\(buf\) \{[\s\S]*?toolMeta\.value = s\.toolMeta \|\| \{\};\n *\}\n/,
  );
  assert.ok(match, 'loadWorkingSetFrom 函数体未找到——抽取正则与源码漂移');
  return match[0];
}

function makeDeferredInvoke(log) {
  const pending = [];
  const invoke = (command, payload) => {
    const call = { command, payload };
    log.push(call);
    return new Promise((resolve, reject) => {
      pending.push({ call, resolve, reject });
    });
  };
  return { invoke, pending };
}

async function drain() {
  // 链上 .then 回调要等微任务队列清空两轮才落日志。
  await new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
  await new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}

function runLoader(sandboxState, reviews) {
  const sandbox = {
    console: { warn: () => {} },
    reportSidecarReadFailure: () => {},
    state: sandboxState,
    async invoke(command) {
      if (command === 'get_session_pinvou_reviews') return reviews;
      return null;
    },
  };
  vm.runInNewContext(
    `${extractLoaderBlock(sharedBaseSource)}\nthis.__loadReviews = loadPinvouReviewsForSession;`,
    sandbox,
    { filename: 'pinvou-summon-seq-loader.js' },
  );
  return sandbox.__loadReviews('s1');
}

test('seq counter seeds from persisted sidecar seqs on hydrate (r5 M1)', async () => {
  const state = { pinvouSummonSeq: 0 };
  await runLoader(state, [
    { pos: 1, seq: 2, review: {} },
    { pos: 2, seq: 7, review: {} },
    { pos: 3, review: {} }, // legacy entry without seq
  ]);
  assert.strictEqual(
    state.pinvouSummonSeq,
    7,
    'counter must dominate the max persisted seq or Rust max_by_key elects a stale pass as latest and every click re-reviews',
  );
});

test('seq seeding ignores non-numeric persisted seq values', async () => {
  const state = { pinvouSummonSeq: 0 };
  await runLoader(state, [{ pos: 1, seq: '9', review: {} }, { pos: 2, seq: true, review: {} }]);
  assert.strictEqual(
    state.pinvouSummonSeq,
    0,
    'string/boolean seq must not poison the numeric counter (hand-edited or older sidecar formats)',
  );
});

test('seq seeding never decrements the counter', async () => {
  const state = { pinvouSummonSeq: 5 };
  await runLoader(state, [{ pos: 1, seq: 2, review: {} }]);
  assert.strictEqual(state.pinvouSummonSeq, 5, 'global max is monotonic across session switches');
});

test('review save chain serializes: later records queue behind pending saves (r5 m6)', async () => {
  const log = [];
  const { invoke, pending } = makeDeferredInvoke(log);
  const state = { activeSessionId: 's1', messages: [{}, {}, {}], pinvouReviews: [] };
  const sandbox = { invoke, state, notify: () => {}, console };
  vm.runInNewContext(
    `${extractSaveChainBlock(sharedBaseSource)}\n` +
      'this.__record = recordPinvouReview;\nthis.__persist = persistPinvouReviews;',
    sandbox,
    { filename: 'pinvou-save-chain.js' },
  );

  const posA = sandbox.__record({ verdict: 'fail', issues: [{ text: 'A' }] }, 1);
  assert.strictEqual(posA, 3, 'record returns the entry pos');
  await drain();
  assert.strictEqual(log.length, 1, 'first settle save starts (chain head is a microtask)');

  // THE chain invariant: while save #1 is pending, the second settle's save
  // must be queued, not fired. Parallel fire-and-forget calls invoke
  // synchronously inside record → this count would be 2 and the older
  // snapshot could land last, permanently dropping the newer entry.
  const posB = sandbox.__record({ verdict: 'pass', issues: [] }, 2);
  assert.strictEqual(posB, 3, 'same transcript length → same pos');
  assert.strictEqual(
    log.length,
    1,
    'second save must wait for the first — parallel snapshot writes are the lost-entry race',
  );

  pending[0].resolve(null);
  await drain();
  assert.strictEqual(log.length, 2, 'queued save runs once the chain head settles');
  assert.strictEqual(log[0].command, 'save_session_pinvou_reviews');
  assert.strictEqual(
    log[0].payload.reviews.length,
    1,
    'snapshot is taken synchronously at record time (first save carries only entry A)',
  );
  assert.strictEqual(
    log[1].payload.reviews.length,
    2,
    'last write is the superset — no entry lost',
  );
  assert.strictEqual(log[1].payload.reviews[1].seq, 2, 'seq rides the persisted entry');
  pending[1].resolve(null);
  await drain();
});

test('persistPinvouReviews rides the same chain behind a pending settle save', async () => {
  const log = [];
  const { invoke, pending } = makeDeferredInvoke(log);
  const state = { activeSessionId: 's1', messages: [{}], pinvouReviews: [] };
  const sandbox = { invoke, state, notify: () => {}, console };
  vm.runInNewContext(
    `${extractSaveChainBlock(sharedBaseSource)}\n` +
      'this.__record = recordPinvouReview;\nthis.__persist = persistPinvouReviews;',
    sandbox,
    { filename: 'pinvou-save-chain-persist.js' },
  );

  sandbox.__record({ verdict: 'fail', issues: [{ text: 'A' }] }, 1);
  await drain();
  const persisted = sandbox.__persist();
  assert.strictEqual(log.length, 1, 'resolution save queues behind the in-flight settle save');
  pending[0].resolve(null);
  await drain();
  assert.strictEqual(log.length, 2, 'resolution save fires after the settle save');
  assert.strictEqual(
    log[1].payload.reviews.length,
    1,
    'resolution snapshot carries the settled entry with its resolution',
  );
  pending[1].resolve(null);
  await drain();
  await persisted; // returns the chain tail — must resolve, not hang
});

test('chained saves keep the sid captured at record time', async () => {
  const log = [];
  const { invoke, pending } = makeDeferredInvoke(log);
  const state = { activeSessionId: 's1', messages: [{}], pinvouReviews: [] };
  const sandbox = { invoke, state, notify: () => {}, console };
  vm.runInNewContext(
    `${extractSaveChainBlock(sharedBaseSource)}\nthis.__record = recordPinvouReview;`,
    sandbox,
    { filename: 'pinvou-save-chain-sid.js' },
  );

  sandbox.__record({ verdict: 'fail', issues: [] }, 1);
  await drain();
  assert.strictEqual(log.length, 1, 'settle save started');
  // Session switches while save #1 is pending: the queued write must still
  // target the session the entry belongs to.
  state.activeSessionId = 's2';
  pending[0].resolve(null);
  await drain();
  assert.strictEqual(
    log[0].payload.sessionId,
    's1',
    'sid is captured synchronously at record time — no cross-session snapshot contamination',
  );
});

// r7 审 m3：settle 门本身此前零自动化覆盖（JS-only 行为，仅注释守护）——重构
// summonPinvou 可能无声重开 r3 审 M9 的两个串扰（旧结果注入新弹窗 / 旧 finally
// 提前解掉新召唤守卫）。抽取真实 summonPinvou + 串行链，在 dismiss→再召唤窗口
// 里让旧 settle 晚到：必须只入账（带自己的 seq），不写新弹窗、不解新守卫；
// 新 settle 到达才写弹窗并解自己的守卫。删任何一处 `seq ===` 门或入账行，本
// 测试红。
test('stale settle records to the ledger but cannot inject into a newer summon nor release its guard', async () => {
  const log = [];
  const { invoke, pending } = makeDeferredInvoke(log);
  const state = {
    activeSessionId: 's1',
    pinvouSummoning: false,
    pinvouSummonSeq: 0,
    pinvouModal: null,
    pinvouReviews: [],
    messages: [{}, {}, {}],
  };
  const sandbox = {
    invoke,
    state,
    notify: () => {},
    console,
    bt: (key) => key,
    addSystemItem: () => {},
  };
  vm.runInNewContext(
    `${extractSaveChainBlock(sharedBaseSource)}\n${extractSummonBlock(sharedBaseSource)}\n` +
      'this.__summon = summonPinvou;\nthis.__dismiss = dismissPinvouReview;',
    sandbox,
    { filename: 'pinvou-summon-settle-gate.js' },
  );

  const first = sandbox.__summon(null, null); // seq 1, in flight
  assert.strictEqual(state.pinvouSummonSeq, 1, 'first summon takes seq 1');
  // Dismiss via the REAL dismissPinvouReview (r9 审 m7：它就在上面抽出的链块
  // span 里，手摆状态会让 dismiss 本体的漂移对测试不可见)。契约：解守卫、关窗、
  // 不取消在途后端请求。
  sandbox.__dismiss();
  assert.strictEqual(state.pinvouSummoning, false, 'dismiss releases the guard');
  assert.strictEqual(state.pinvouModal, null, 'dismiss closes the modal');
  const second = sandbox.__summon(null, null); // seq 2
  assert.strictEqual(state.pinvouSummonSeq, 2, 're-summon takes the next seq');
  assert.strictEqual(state.pinvouSummoning, true, 're-summon re-arms the guard');

  // Stale settle (seq 1) lands late: ledger-only.
  pending[0].resolve({ verdict: 'fail', issues: [{ text: 'stale' }] });
  await drain();
  await drain();
  assert.strictEqual(
    state.pinvouReviews.length,
    1,
    'superseded settle still records to the review ledger (deliberate, disclosed)',
  );
  assert.strictEqual(state.pinvouReviews[0].seq, 1, 'stale entry rides its own seq');
  assert.strictEqual(
    state.pinvouModal && state.pinvouModal.loading && !state.pinvouModal.review,
    true,
    'stale settle must not inject into the newer summon modal (r3 M9 crossover 1: still loading, no review)',
  );
  assert.strictEqual(
    state.pinvouSummoning,
    true,
    'stale finally must not release the newer summon guard (r3 M9 crossover 2)',
  );
  assert.strictEqual(
    log.length,
    3,
    'two summons in flight + the stale settle save fired (ledger-only)',
  );

  // Fresh settle (seq 2): injects and releases its own guard.
  pending[1].resolve({ verdict: 'pass', issues: [] });
  await first;
  await second;
  await drain();
  assert.strictEqual(
    state.pinvouModal && state.pinvouModal.review && state.pinvouModal.review.verdict,
    'pass',
    'fresh settle injects its review into the modal',
  );
  assert.strictEqual(
    state.pinvouSummoning,
    false,
    'fresh settle releases the guard for the next summon',
  );
  assert.strictEqual(
    state.pinvouReviews.length,
    2,
    'both settles are ledgered — the seq gate orders consumption, not persistence',
  );
  pending[2].resolve(null);
  await drain();
  assert.strictEqual(log.length, 4, 'fresh settle save queued behind the stale save (chain)');
  pending[3].resolve(null);
  await drain();
});

// r9 审 m7：catch 分支的序号门此前零覆盖（成功路径有钉、错误路径没有）——
// 被顶掉的旧 settle 若 reject，其错误不得写进新弹窗、不得解新守卫、也不入账
// （recordPinvouReview 只在成功路径调用）。
test('superseded error settle cannot write its error into the newer modal', async () => {
  const log = [];
  const { invoke, pending } = makeDeferredInvoke(log);
  const state = {
    activeSessionId: 's1',
    pinvouSummoning: false,
    pinvouSummonSeq: 0,
    pinvouModal: null,
    pinvouReviews: [],
    messages: [{}, {}, {}],
  };
  const sandbox = {
    invoke,
    state,
    notify: () => {},
    console,
    bt: (key) => key,
    addSystemItem: () => {},
  };
  vm.runInNewContext(
    `${extractSaveChainBlock(sharedBaseSource)}\n${extractSummonBlock(sharedBaseSource)}\n` +
      'this.__summon = summonPinvou;\nthis.__dismiss = dismissPinvouReview;',
    sandbox,
    { filename: 'pinvou-summon-error-gate.js' },
  );

  const first = sandbox.__summon(null, null); // seq 1, in flight
  sandbox.__dismiss();
  const second = sandbox.__summon(null, null); // seq 2
  assert.strictEqual(state.pinvouSummonSeq, 2, 're-summon takes the next seq');

  // Stale settle (seq 1) REJECTS: the catch branch is seq-gated.
  pending[0].reject(new Error('stale backend blew up'));
  await drain();
  await drain();
  assert.strictEqual(
    state.pinvouModal && state.pinvouModal.loading && !state.pinvouModal.error,
    true,
    'stale error must not write into the newer summon modal (catch branch gate)',
  );
  assert.strictEqual(
    state.pinvouSummoning,
    true,
    'stale error finally must not release the newer summon guard',
  );
  assert.strictEqual(
    state.pinvouReviews.length,
    0,
    'error settles record nothing to the ledger (recordPinvouReview is success-path only)',
  );

  // Fresh settle (seq 2) succeeds normally.
  pending[1].resolve({ verdict: 'pass', issues: [] });
  await first;
  await second;
  await drain();
  assert.strictEqual(
    state.pinvouModal && state.pinvouModal.review && state.pinvouModal.review.verdict,
    'pass',
    'fresh settle unaffected by the stale error',
  );
  assert.strictEqual(
    state.pinvouSummoning,
    false,
    'fresh settle releases its own guard',
  );
  pending[2].resolve(null);
  await drain();
});

// r9 审 m7：restore 路径解守卫（r5 修复 `state.pinvouSummoning = false`）此前
// 只有注释守护，删行全绿。行为级复放真实 loadWorkingSetFrom：守卫被解、弹窗被
// 关、reviews 数组按 buffer rebind（hydrate rebind 语义，同款是 save 链注释
// 记载的丢失窗口来源）。
test('session-buffer restore releases the summon guard and closes the modal (r5 fix pin)', async () => {
  const state = {
    pinvouSummoning: true,
    pinvouModal: { loading: true },
    pinvouReviews: [{ pos: 1, review: {} }],
    messages: [],
    chatItems: [],
    artifacts: [],
    composerDraft: '',
    turnTimeline: [],
    activeTurnTimelineId: null,
    personaEvents: [],
    pinvouSceneEvents: [],
    turnDirtyArtifacts: [],
    turnPresentedArtifacts: [],
    busy: true,
    queued: [],
    activePersona: null,
    mountedCollection: null,
    mountedCollections: [],
    mountedCollectionsRevision: 0,
    planSnapshot: null,
    modeState: null,
    thinking: null,
    tokens: null,
  };
  const sandbox = {
    state,
    restoreSteeredWorkingSet: null,
    currentStreamText: { value: 'x' },
    currentStreamId: { value: 1 },
    pendingAssistantText: { value: 'x' },
    pendingAssistantBlocks: { value: [{}] },
    itemIdSeq: { value: 5 },
    toolMeta: { value: {} },
  };
  vm.runInNewContext(
    `${extractRestoreBlock(sharedBaseSource)}\nthis.__restore = loadWorkingSetFrom;`,
    sandbox,
    { filename: 'pinvou-restore-gate.js' },
  );
  sandbox.__restore({
    messages: [],
    chatItems: [],
    artifacts: [],
    pinvouReviews: [{ pos: 9, review: {} }],
    stream: {},
  });
  assert.strictEqual(
    state.pinvouSummoning,
    false,
    'restore must release the summon guard (r5: otherwise 品/悟 in the new session silently swallowed up to 180s)',
  );
  assert.strictEqual(
    state.pinvouModal,
    null,
    'restore closes the review modal (session switch)',
  );
  assert.strictEqual(
    state.pinvouReviews.length,
    1,
    'restore rebinds reviews to the session buffer array',
  );
  assert.strictEqual(
    state.pinvouReviews[0].pos,
    9,
    'rebind takes the buffer array wholesale (hydrate rebind semantics)',
  );
});
