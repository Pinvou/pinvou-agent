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

// 后台装载抬底的两个落点在 lane 里（共享 raise 本体仍在 helpers）：抽 tauri
// sessions.js 的 ensureSessionBufferLoaded 真身（web bridge.js 逐字镜像同一
// 抬底行），与共享 loader/raise/summon 块同 ctx 执行，复放真实后台交错。
const sessionsSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'sessions.js'),
  'utf8',
);
const webBridgeSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'platform', 'web', 'bridge.js'),
  'utf8',
);

function extractEnsureBlock(source) {
  const match = source.match(
    /async function ensureSessionBufferLoaded\(sid\) \{[\s\S]*?buf\.loadedFromDisk = true;\n  \}\n/,
  );
  assert.ok(match, 'ensureSessionBufferLoaded 函数体未找到——抽取正则与源码漂移');
  return match[0];
}

function extractRaiseBlock(source) {
  const match = source.match(
    /function raisePinvouSummonHighwater\(reviews\) \{[\s\S]*?\n  \}\n/,
  );
  assert.ok(match, 'raisePinvouSummonHighwater 函数体未找到——抽取正则与源码漂移');
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

function runRaise(sandboxState, reviews) {
  const match = sharedBaseSource.match(
    /function raisePinvouSummonHighwater\(reviews\) \{[\s\S]*?\n  \}\n/,
  );
  assert.ok(match, 'raisePinvouSummonHighwater 函数体未找到——抽取正则与源码漂移');
  const sandbox = { state: sandboxState };
  vm.runInNewContext(
    `${match[0]}\nthis.__raise = raisePinvouSummonHighwater;`,
    sandbox,
    { filename: 'pinvou-summon-seq-raise.js' },
  );
  sandbox.__raise(reviews);
}

test('seq counter seeds from persisted sidecar seqs at the acceptance commit (r5 M1)', async () => {
  const state = { pinvouSummonSeq: 0 };
  runRaise(state, [
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
  runRaise(state, [{ pos: 1, seq: '9', review: {} }, { pos: 2, seq: true, review: {} }]);
  assert.strictEqual(
    state.pinvouSummonSeq,
    0,
    'string/boolean seq must not poison the numeric counter (hand-edited or older sidecar formats)',
  );
});

test('seq seeding never decrements the counter', async () => {
  const state = { pinvouSummonSeq: 5 };
  runRaise(state, [{ pos: 1, seq: 2, review: {} }]);
  assert.strictEqual(state.pinvouSummonSeq, 5, 'global max is monotonic across session switches');
});

test('loader itself no longer raises the counter — commits happen only at acceptance (zhuowp round-2 P2-2)', async () => {
  const state = { pinvouSummonSeq: 0 };
  await runLoader(state, [{ pos: 1, seq: 99, review: {} }]);
  assert.strictEqual(
    state.pinvouSummonSeq,
    0,
    'a rejected navigation replays this loader after the raise used to happen; raising here poisons every in-flight summon seq gate',
  );
});

test('rejected-navigation loader cannot poison an in-flight summon (zhuowp round-2 P2-2)', async () => {
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
    `${extractLoaderBlock(sharedBaseSource)}\n${extractSaveChainBlock(sharedBaseSource)}\n${extractSummonBlock(sharedBaseSource)}\n` +
      'this.__loadReviews = loadPinvouReviewsForSession;\nthis.__summon = summonPinvou;',
    sandbox,
    { filename: 'pinvou-rejected-nav-interleaving.js' },
  );

  // B's sidecar load starts (the would-be navigation; its token is still
  // unchecked at this point).
  const loaderDone = sandbox.__loadReviews('s2');
  // The user summons on the still-active A while B's load is in flight.
  const summonDone = sandbox.__summon(null, null);
  assert.strictEqual(state.pinvouSummonSeq, 1, 'the summon takes seq 1');
  await drain();
  // B's load resolves with a HIGHER persisted seq. The old loader raised the
  // global here — before the navigation was rejected — and A's settle then
  // found `seq !== state.pinvouSummonSeq`: the modal stayed loading and the
  // guard never released. With the raise moved to the acceptance point, the
  // rejected load must leave the gate untouched.
  pending.find((p) => p.call.command === 'get_session_pinvou_reviews').resolve([
    { pos: 1, seq: 99, review: {} },
  ]);
  await loaderDone;
  assert.strictEqual(
    state.pinvouSummonSeq,
    1,
    'the rejected load must not raise the global counter',
  );
  // A's settle lands: the gate still matches, the modal unlocks.
  pending.find((p) => p.call.command === 'summon_pinvou').resolve({ verdict: 'pass', issues: [] });
  await drain();
  await summonDone;
  assert.strictEqual(
    state.pinvouModal && state.pinvouModal.loading === false &&
      state.pinvouModal.review && state.pinvouModal.review.verdict,
    'pass',
    'the active summon must still settle its modal',
  );
  assert.strictEqual(state.pinvouSummoning, false, 'the guard must release');
  assert.strictEqual(
    state.pinvouReviews.length,
    1,
    'the settle is still ledgered with its own seq',
  );
});

// JensenChen28 审 P2：所有权与账目序解耦前的缺陷复放。全局 pinvouSummonSeq 有
// 两个角色：(1) 账目抬底高位水（sidecar 持久 seq，重启清零后靠装载抬回，
// r5 审 M1，保留）；(2) 在途召唤的所有权令牌（settle 凭它写弹窗/解守卫）。
// 活动会话 A 召唤在途时，后台 ensureSessionBufferLoaded（sid !== active 的
// 非活跃装载）抬 s2 sidecar 的高位水——s2 持久 seq 更大就把全局抬过 A 捕获的
// seq，旧门 `seq === state.pinvouSummonSeq` 从此不等：A 的成功响应不再结算
// 弹窗、也不解守卫，活动弹窗卡 loading 到关窗为止。解耦后门走
// pinvouSummonTicket（只有召唤流程自增，抬底碰不到），同样的交错必须照常
// 结算。把门改回 seq、删掉 ticket 自增、或让抬底顺手动 ticket，本测试红。
test('background hydrate raising the ledger seq cannot strand an in-flight summon (JensenChen28 review P2)', async () => {
  const log = [];
  const { invoke, pending } = makeDeferredInvoke(log);
  const state = {
    activeSessionId: 's1',
    pinvouSummoning: false,
    pinvouSummonSeq: 0,
    pinvouModal: null,
    pinvouReviews: [],
    messages: [{}, {}, {}],
    sessions: [{ id: 's2', message_count: 4 }],
  };
  const buffers = {
    s2: { messages: [], chatItems: [], loadedFromDisk: false, busy: false, remoteTurnActive: false },
  };
  const sandbox = {
    invoke,
    state,
    notify: () => {},
    console,
    bt: (key) => key,
    addSystemItem: () => {},
    getBuffer: (sid) => buffers[sid],
    hydrateWorkingSetFromSaved: () => {},
    loadPersonaEventsForSession: async () => [],
    syncPinvouSceneEventsForSession: async () => [],
    syncSteeredMessagesForSession: async () => [],
    runSyncOnSession: (sid, fn) => fn(),
    resetPendingAssistant: () => {},
    rerenderFromMessages: () => {},
  };
  vm.runInNewContext(
    `${extractLoaderBlock(sharedBaseSource)}\n${extractRaiseBlock(sharedBaseSource)}\n${extractSaveChainBlock(sharedBaseSource)}\n${extractSummonBlock(sharedBaseSource)}\n${extractEnsureBlock(sessionsSource)}\n` +
      'this.__summon = summonPinvou;\nthis.__ensure = ensureSessionBufferLoaded;',
    sandbox,
    { filename: 'pinvu-summon-ticket-interleaving.js' },
  );

  // A（活动会话 s1）召唤在途：捕获 seq 1 / ticket 1。
  const summonDone = sandbox.__summon(null, null);
  assert.strictEqual(state.pinvouSummonSeq, 1, 'the summon takes seq 1');
  assert.strictEqual(state.pinvouSummoning, true, 'the summon arms the guard');
  // 后台装载非活跃 s2（真身 ensureSessionBufferLoaded，含 sid !== active 门）。
  const ensureDone = sandbox.__ensure('s2');
  await drain();
  pending.find((p) => p.call.command === 'load_session').resolve({
    messages: [{}, {}, {}, {}],
    metadata: { id: 's2', message_count: 4 },
  });
  await drain();
  pending.find((p) => p.call.command === 'get_session_pinvou_reviews').resolve([
    { pos: 1, seq: 99, review: {} },
  ]);
  await drain();
  pending.find((p) => p.call.command === 'get_session_timeline').resolve([]);
  await ensureDone;
  // 缺陷前提成立：后台抬底确实把全局账目序抬过了在途召唤的 seq——旧实现此处
  // 之后 A 永远结算不了。ticket 必须纹丝不动。
  assert.strictEqual(
    state.pinvouSummonSeq,
    99,
    'the background raise must happen (r5 M1 ledger intent kept) and must pass the in-flight seq',
  );
  assert.strictEqual(
    state.pinvouSummonTicket,
    1,
    'the raise is ordering-only: it must never touch the ownership ticket',
  );
  // A 的响应此刻才回来：ticket 门仍匹配 → 弹窗照常解锁、守卫照常放行。
  pending.find((p) => p.call.command === 'summon_pinvou').resolve({ verdict: 'pass', issues: [] });
  await drain();
  await summonDone;
  assert.strictEqual(
    state.pinvouModal && state.pinvouModal.loading === false &&
      state.pinvouModal.review && state.pinvouModal.review.verdict,
    'pass',
    'the in-flight summon must still settle its modal after the background raise',
  );
  assert.strictEqual(state.pinvouSummoning, false, 'the guard must release');
  assert.strictEqual(
    state.pinvouReviews[0].seq,
    1,
    'the ledger entry rides its own issuance seq, not the highwater',
  );
  pending.find((p) => p.call.command === 'save_session_pinvou_reviews').resolve(null);
  await drain();
});

// JensenChen28 审 P2 次要面（排序半边）：切换路径的抬底必须落在 saved.metadata
// 校验之后——被拒/畸形装载不得抬账目底（web 桥曾把 raise 放在校验之前，回归
// 过的真实形态）。行为级无法驱动两个 lane 的巨型 switchToSessionInternal（依
// 赖面太大），按 bridge_lane_deps 的结构钉风格锁源码序：抬底调用点必须晚于
// metadata 校验、早于提交点。
test('switch-path highwater raise sits after the saved.metadata validation in both lanes', () => {
  for (const [label, laneSource] of [['tauri sessions.js', sessionsSource], ['web bridge.js', webBridgeSource]]) {
    const fnStart = laneSource.indexOf('async function switchToSessionInternal(');
    assert.ok(fnStart >= 0, `${label}: switchToSessionInternal 未找到`);
    const commitMark = laneSource.indexOf('才一次性提交 active/context', fnStart);
    assert.ok(commitMark > fnStart, `${label}: 提交点注释未找到`);
    const span = laneSource.slice(fnStart, commitMark);
    const validateMark = span.indexOf('!saved.metadata || !saved.metadata.id');
    const raiseMark = span.indexOf('raisePinvouSummonHighwater(pinvouReviews)');
    assert.ok(validateMark >= 0, `${label}: saved.metadata 校验未找到`);
    assert.ok(raiseMark >= 0, `${label}: 切换路径抬底未找到`);
    assert.ok(
      raiseMark > validateMark,
      `${label}: 切换路径抬底必须落在 saved.metadata 校验之后（被拒装载不得抬账目底，JensenChen28 审 P2）`,
    );
  }
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
