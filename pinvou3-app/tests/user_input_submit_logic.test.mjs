#!/usr/bin/env node
// request_user_input 卡片提交链路的纯逻辑回归（PR #220 修复审计）：
//   1) submitUserInput 摘要按 question 分组拼接——multi_select 时 answers 按选项展开，
//      不能按 answers 索引一一对应 questions（旧实现会越界抛 TypeError，restoredAnswers 丢失）。
//   2) submitUserInput/cancelUserInput 跨 await 后 UI 写入必须定向到触发会话 sid
//      （runOnSession / patchItemByIdFor），用户提交期间切会话不得漏写/污染。
//   3) 两个 bridge 的 parseUserAnswers 按 id 分组保留全部同 id 答案（多选不塌缩），
//      与 code-native-lane 的 parseNativeUserAnswers 对齐。
// 风格对齐 chat_turn_error_isolation.test.mjs：vm 加载 IIFE 源文件 + mock context。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

// ── interaction.js（tauri feature）─────────────────────────────────
const sandbox = { window: {} };
vm.runInNewContext(read('src', 'platform', 'tauri', 'bridge', 'interaction.js'), sandbox, {
  filename: 'interaction.js',
});
const installInteraction = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__.interaction;

// 按 sid 模拟 runSyncOnSession 的 buffer swap：把 state 工作集临时切到目标 session。
// 真实实现（tauri bridge.js:973）是 swap-load-fn-save；这里用数组引用模拟，足以断言
// "UI 写入落在触发会话而非当前会话"。
function makeContext(initialState) {
  const buffers = {};
  const state = Object.assign({ activeSessionId: null, chatItems: [], messages: [] }, initialState);
  const runOnSessionCalls = [];
  const context = {
    state,
    buffers,
    runOnSessionCalls,
    invoke: async () => ({}),
    notify() {},
    bt(key) {
      return ({ echoOtherPrefix: '(其他) ' })[key] || key;
    },
    addSystemItem() {},
    addChatItem(item) { state.chatItems.push(item); },
    timeStr: () => '',
    runSyncOnSession(sid, fn) {
      runOnSessionCalls.push(sid);
      if (!sid || sid === state.activeSessionId) { fn(); return; }
      const bg = buffers[sid];
      if (!bg) return;
      const realChatItems = state.chatItems;
      const realMessages = state.messages;
      const realId = state.activeSessionId;
      state.chatItems = bg.chatItems;
      state.messages = bg.messages;
      state.activeSessionId = sid;
      try { fn(); }
      finally {
        bg.chatItems = state.chatItems;
        bg.messages = state.messages;
        state.chatItems = realChatItems;
        state.messages = realMessages;
        state.activeSessionId = realId;
      }
    },
    flushAssistantMessageToHistory() {},
    resetPendingAssistant() {},
    rerenderFromMessages() {},
    turnUsageDirty: {},
    ensureSession: async () => 'session-1',
    sendMessage: async () => {},
    getBuffer() { return {}; },
    reconcileRemoteTurn: async () => true,
    isBusyFor: () => false,
    markRemoteTurn() {},
    get currentStreamText() { return ''; },
    set currentStreamText(v) {},
    get currentStreamId() { return 0; },
    set currentStreamId(v) {},
    get itemIdSeq() { return 1; },
    set itemIdSeq(v) {},
  };
  return context;
}

// ── 单选提交：摘要与 restoredAnswers ───────────────────────────────
{
  const ctx = makeContext({ activeSessionId: 'session-1' });
  ctx.state.chatItems = [{ id: 'card-1', type: 'user_input', toolCallId: 'call-1', questions: [{ id: 'q1', header: '语言' }] }];
  const feature = installInteraction(ctx);
  await feature.submitUserInput(
    'card-1',
    'call-1',
    [{ id: 'q1', label: 'Python', value: 'Python' }],
    [{ id: 'q1', header: '语言' }],
  );
  const card = ctx.state.chatItems[0];
  assert.equal(card.resolved, true);
  assert.equal(card.cardState, 'submitted');
  assert.deepEqual(card.restoredAnswers, [{ id: 'q1', label: 'Python', value: 'Python' }]);
  assert.ok(ctx.state.chatItems.some(item => item.type === 'user' && item.text === '✓ 语言: Python'), '单选摘要按题头拼接');
}

// ── 多选提交：单题选两项，摘要不越界、restoredAnswers 全量 ────────
{
  const ctx = makeContext({ activeSessionId: 'session-1' });
  ctx.state.chatItems = [{ id: 'card-2', type: 'user_input', toolCallId: 'call-2', questions: [{ id: 'q1', header: '技能' }] }];
  const feature = installInteraction(ctx);
  // 多选展开：同一 question 两条答案，answers.length(2) > questions.length(1)。
  await feature.submitUserInput(
    'card-2',
    'call-2',
    [
      { id: 'q1', label: '前端', value: '前端' },
      { id: 'q1', label: '运维', value: '运维' },
    ],
    [{ id: 'q1', header: '技能' }],
  );
  const card = ctx.state.chatItems[0];
  assert.equal(card.resolved, true, '多选提交不得因摘要越界抛错');
  assert.deepEqual(card.restoredAnswers, [
    { id: 'q1', label: '前端', value: '前端' },
    { id: 'q1', label: '运维', value: '运维' },
  ], '多选答案全量保存，不塌缩');
  const echo = ctx.state.chatItems.find(item => item.type === 'user');
  assert.ok(echo && echo.text === '✓ 技能: 前端 · 运维', `多选摘要按题分组：实际 "${echo && echo.text}"`);
}

// ── 多选提交：两题混合（q1 单选 + q2 多选），摘要按 questions 顺序分组 ──
{
  const ctx = makeContext({ activeSessionId: 'session-1' });
  ctx.state.chatItems = [{ id: 'card-3', type: 'user_input', toolCallId: 'call-3', questions: [
    { id: 'q1', header: '语言' },
    { id: 'q2', header: '技能' },
  ] }];
  const feature = installInteraction(ctx);
  await feature.submitUserInput(
    'card-3',
    'call-3',
    [
      { id: 'q1', label: 'Python', value: 'Python' },
      { id: 'q2', label: '前端', value: '前端' },
      { id: 'q2', label: '运维', value: '运维' },
    ],
    [
      { id: 'q1', header: '语言' },
      { id: 'q2', header: '技能' },
    ],
  );
  const echo = ctx.state.chatItems.find(item => item.type === 'user');
  assert.equal(echo.text, '✓ 语言: Python · 技能: 前端 · 运维');
}

// ── 切会话竞态：invoke 挂起期间切换 activeSessionId，写入仍落触发会话 ──
{
  const ctx = makeContext({ activeSessionId: 'session-A' });
  ctx.buffers['session-A'] = { chatItems: [{ id: 'card-A', type: 'user_input', toolCallId: 'call-A', questions: [{ id: 'q1', header: '语言' }] }], messages: [] };
  ctx.buffers['session-B'] = { chatItems: [{ id: 'card-B', type: 'user_input', toolCallId: 'call-B', questions: [{ id: 'q1', header: '语言' }] }], messages: [] };
  // 用 state 工作集指向 A（active），B 只存在 buffers 中。
  ctx.state.chatItems = ctx.buffers['session-A'].chatItems;
  ctx.state.messages = ctx.buffers['session-A'].messages;

  let resolveInvoke;
  const gate = new Promise(resolve => { resolveInvoke = resolve; });
  ctx.invoke = async () => { await gate; return {}; };
  const feature = installInteraction(ctx);

  const pending = feature.submitUserInput(
    'card-A',
    'call-A',
    [{ id: 'q1', label: 'Python', value: 'Python' }],
    [{ id: 'q1', header: '语言' }],
  );
  // invoke 未返回时用户切到 B。
  ctx.state.activeSessionId = 'session-B';
  ctx.state.chatItems = ctx.buffers['session-B'].chatItems;
  ctx.state.messages = ctx.buffers['session-B'].messages;
  resolveInvoke();
  await pending;

  assert.ok(ctx.runOnSessionCalls.every(sid => sid === 'session-A'), '切会话后 UI 写入全部定向到触发会话 A');
  const cardA = ctx.buffers['session-A'].chatItems[0];
  assert.equal(cardA.resolved, true, 'A 的卡收到提交结果');
  assert.equal(cardA.cardState, 'submitted');
  assert.deepEqual(cardA.restoredAnswers, [{ id: 'q1', label: 'Python', value: 'Python' }], 'restoredAnswers 写回 A');
  assert.ok(ctx.buffers['session-A'].chatItems.some(item => item.type === 'user'), 'echo 落在 A');
  const cardB = ctx.buffers['session-B'].chatItems[0];
  assert.equal(cardB.resolved, undefined, 'B 的卡不受污染');
  assert.equal(ctx.buffers['session-B'].chatItems.some(item => item.type === 'user'), false, 'B 不出现 echo');
}

// ── cancelUserInput：同样定向到触发会话 ────────────────────────────
{
  const ctx = makeContext({ activeSessionId: 'session-A' });
  ctx.buffers['session-A'] = { chatItems: [{ id: 'card-A', type: 'user_input', toolCallId: 'call-A' }], messages: [] };
  ctx.state.chatItems = ctx.buffers['session-A'].chatItems;
  ctx.state.messages = ctx.buffers['session-A'].messages;
  let resolveInvoke;
  const gate = new Promise(resolve => { resolveInvoke = resolve; });
  ctx.invoke = async () => { await gate; };
  const feature = installInteraction(ctx);
  const pending = feature.cancelUserInput('card-A', 'call-A');
  ctx.state.activeSessionId = 'session-B';
  resolveInvoke();
  await pending;
  assert.ok(ctx.runOnSessionCalls.every(sid => sid === 'session-A'));
  assert.equal(ctx.buffers['session-A'].chatItems[0].cardState, 'cancelled', '取消结果写回 A');
}

// ── 特殊 question id（constructor/toString/__proto__）不触发原型链 TypeError ──
// request_user_input 的 question id 后端仅校验非空，模型生成这些保留属性名是合法输入。
// 修复前 `(byId[a.id] = byId[a.id] || []).push(a)` 会命中 Object.prototype 继承属性
// （byId['constructor'] 是 Object 构造器、byId['toString'] 是函数、byId['__proto__']
// 是原型对象，均无 .push），提交路径抛 TypeError 且 restoredAnswers 丢失（复核 P1）。
{
  for (const specialId of ['constructor', 'toString', '__proto__']) {
    const ctx = makeContext({ activeSessionId: 'session-1' });
    ctx.state.chatItems = [{ id: 'card-s', type: 'user_input', toolCallId: 'call-s', questions: [{ id: specialId, header: '选择' }] }];
    const feature = installInteraction(ctx);
    await feature.submitUserInput(
      'card-s',
      'call-s',
      [{ id: specialId, label: 'A', value: 'A' }],
      [{ id: specialId, header: '选择' }],
    );
    const card = ctx.state.chatItems[0];
    assert.equal(card.resolved, true, `特殊 id "${specialId}" 提交不得抛错`);
    assert.equal(card.cardState, 'submitted');
    assert.deepEqual(card.restoredAnswers, [{ id: specialId, label: 'A', value: 'A' }], `restoredAnswers 写入特殊 id "${specialId}"`);
    const echo = ctx.state.chatItems.find(item => item.type === 'user');
    assert.ok(echo && echo.text === `✓ 选择: A`, `特殊 id "${specialId}" 摘要正常`);
  }
}

// ── parseUserAnswers：两个 bridge 多选不塌缩 ────────────────────────
// parseUserAnswers 是 bridge 主文件闭包内函数，用括号配平提取函数体到 vm 执行。
function extractFunction(source, name) {
  const start = source.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`function ${name} not found`);
  const open = source.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < source.length; i++) {
    if (source[i] === '{') depth += 1;
    else if (source[i] === '}') { depth -= 1; if (depth === 0) return source.slice(start, i + 1); }
  }
  throw new Error(`function ${name} braces unbalanced`);
}

function loadParseUserAnswers(bridgeSource) {
  const ctx = {};
  vm.createContext(ctx);
  const code = [
    extractFunction(bridgeSource, 'toolResultText'),
    extractFunction(bridgeSource, 'parseUserAnswers'),
    'this.parseUserAnswers = parseUserAnswers;',
  ].join('\n');
  vm.runInContext(code, ctx);
  // vm 跨 realm 对象原型不同，deepEqual 会因引用不等失败，统一 JSON 规范化后断言。
  return (content, questions) => JSON.parse(JSON.stringify(ctx.parseUserAnswers(content, questions)));
}

// ── web bridge：submitUserInput/cancelUserInput 行为契约 ─────────────
// 评审 P2-2：此前只执行 Tauri interaction.js 的提交链路，Web 侧仅覆盖
// parseUserAnswers。这里提取 web/bridge.js 闭包内的提交函数，用与 interaction
// 侧相同的 mock 环境（buffer swap + sid 定向）跑同一套行为契约：单选/多选摘要、
// 切会话竞态、取消定向、特殊 id 提交。
function makeWebContext(initialState) {
  const buffers = {};
  const state = Object.assign({ activeSessionId: null, chatItems: [], messages: [] }, initialState);
  const runOnSessionCalls = [];
  const context = {
    state,
    buffers,
    runOnSessionCalls,
    invoke: async () => ({}),
    notify() {},
    runOnSession(sid, fn) {
      runOnSessionCalls.push(sid);
      if (!sid || sid === state.activeSessionId) { fn(); return; }
      const bg = buffers[sid];
      if (!bg) return;
      const realChatItems = state.chatItems;
      const realMessages = state.messages;
      const realId = state.activeSessionId;
      state.chatItems = bg.chatItems;
      state.messages = bg.messages;
      state.activeSessionId = sid;
      try { fn(); }
      finally {
        bg.chatItems = state.chatItems;
        bg.messages = state.messages;
        state.chatItems = realChatItems;
        state.messages = realMessages;
        state.activeSessionId = realId;
      }
    },
    patchItemByIdFor(sid, itemId, patch) {
      context.runOnSession(sid, () => {
        const item = state.chatItems.find(candidate => candidate.id === itemId);
        if (item) Object.assign(item, patch);
      });
    },
    pushUserEcho(text, persist) {
      state.chatItems.push({ type: 'user', text, persist });
    },
    flushAssistantMessageToHistory() {},
  };
  return context;
}

function loadWebUserInputActions(bridgeSource) {
  const ctx = {};
  vm.createContext(ctx);
  // web bridge 的提交函数带 async 前缀，extractFunction 按 `function name(` 定位会
  // 丢掉 async，导致 vm 里 await 非法；检测源码前缀后补回。
  const asyncOf = name => bridgeSource.includes(`async function ${name}(`) ? 'async ' : '';
  const code = [
    asyncOf('submitUserInput') + extractFunction(bridgeSource, 'submitUserInput'),
    asyncOf('cancelUserInput') + extractFunction(bridgeSource, 'cancelUserInput'),
    'this.submitUserInput = submitUserInput;',
    'this.cancelUserInput = cancelUserInput;',
  ].join('\n');
  vm.runInContext(code, ctx);
  return ctx;
}

const webBridgeSource = read('src', 'platform', 'web', 'bridge.js');
const webActions = loadWebUserInputActions(webBridgeSource);

// ── Web 单选提交：摘要与 restoredAnswers ───────────────────────────
{
  const ctx = makeWebContext({ activeSessionId: 'session-1' });
  Object.assign(webActions, ctx);
  ctx.state.chatItems = [{ id: 'wcard-1', type: 'user_input', toolCallId: 'call-1', questions: [{ id: 'q1', header: '语言' }] }];
  await webActions.submitUserInput(
    'wcard-1',
    'call-1',
    [{ id: 'q1', label: 'Python', value: 'Python' }],
    [{ id: 'q1', header: '语言' }],
  );
  const card = ctx.state.chatItems[0];
  assert.equal(card.resolved, true, 'web 单选提交 resolved');
  assert.equal(card.cardState, 'submitted');
  assert.deepEqual(card.restoredAnswers, [{ id: 'q1', label: 'Python', value: 'Python' }], 'web 单选 restoredAnswers');
  assert.ok(ctx.state.chatItems.some(item => item.type === 'user' && item.text === '✓ 语言: Python'), 'web 单选摘要按题头拼接');
}

// ── Web 多选提交：单题选两项，摘要不越界、restoredAnswers 全量 ─────
{
  const ctx = makeWebContext({ activeSessionId: 'session-1' });
  Object.assign(webActions, ctx);
  ctx.state.chatItems = [{ id: 'wcard-2', type: 'user_input', toolCallId: 'call-2', questions: [{ id: 'q1', header: '技能' }] }];
  await webActions.submitUserInput(
    'wcard-2',
    'call-2',
    [
      { id: 'q1', label: '前端', value: '前端' },
      { id: 'q1', label: '运维', value: '运维' },
    ],
    [{ id: 'q1', header: '技能' }],
  );
  const card = ctx.state.chatItems[0];
  assert.equal(card.resolved, true, 'web 多选提交不得因摘要越界抛错');
  assert.deepEqual(card.restoredAnswers, [
    { id: 'q1', label: '前端', value: '前端' },
    { id: 'q1', label: '运维', value: '运维' },
  ], 'web 多选答案全量保存，不塌缩');
  const echo = ctx.state.chatItems.find(item => item.type === 'user');
  assert.ok(echo && echo.text === '✓ 技能: 前端 · 运维', `web 多选摘要按题分组：实际 "${echo && echo.text}"`);
}

// ── Web 切会话竞态：invoke 挂起期间切换 activeSessionId，写入仍落触发会话 ──
{
  const ctx = makeWebContext({ activeSessionId: 'session-A' });
  Object.assign(webActions, ctx);
  ctx.buffers['session-A'] = { chatItems: [{ id: 'wcard-A', type: 'user_input', toolCallId: 'call-A', questions: [{ id: 'q1', header: '语言' }] }], messages: [] };
  ctx.buffers['session-B'] = { chatItems: [{ id: 'wcard-B', type: 'user_input', toolCallId: 'call-B', questions: [{ id: 'q1', header: '语言' }] }], messages: [] };
  ctx.state.chatItems = ctx.buffers['session-A'].chatItems;
  ctx.state.messages = ctx.buffers['session-A'].messages;

  let resolveInvoke;
  const gate = new Promise(resolve => { resolveInvoke = resolve; });
  ctx.invoke = async () => { await gate; return {}; };
  const pending = webActions.submitUserInput(
    'wcard-A',
    'call-A',
    [{ id: 'q1', label: 'Python', value: 'Python' }],
    [{ id: 'q1', header: '语言' }],
  );
  ctx.state.activeSessionId = 'session-B';
  ctx.state.chatItems = ctx.buffers['session-B'].chatItems;
  ctx.state.messages = ctx.buffers['session-B'].messages;
  resolveInvoke();
  await pending;

  assert.ok(ctx.runOnSessionCalls.every(sid => sid === 'session-A'), 'web 切会话后 UI 写入全部定向到触发会话 A');
  const cardA = ctx.buffers['session-A'].chatItems[0];
  assert.equal(cardA.resolved, true, 'web A 的卡收到提交结果');
  assert.equal(cardA.cardState, 'submitted');
  assert.deepEqual(cardA.restoredAnswers, [{ id: 'q1', label: 'Python', value: 'Python' }], 'web restoredAnswers 写回 A');
  assert.ok(ctx.buffers['session-A'].chatItems.some(item => item.type === 'user'), 'web echo 落在 A');
  const cardB = ctx.buffers['session-B'].chatItems[0];
  assert.equal(cardB.resolved, undefined, 'web B 的卡不受污染');
  assert.equal(ctx.buffers['session-B'].chatItems.some(item => item.type === 'user'), false, 'web B 不出现 echo');
}

// ── Web cancelUserInput：同样定向到触发会话 ────────────────────────
{
  const ctx = makeWebContext({ activeSessionId: 'session-A' });
  Object.assign(webActions, ctx);
  ctx.buffers['session-A'] = { chatItems: [{ id: 'wcard-A', type: 'user_input', toolCallId: 'call-A' }], messages: [] };
  ctx.state.chatItems = ctx.buffers['session-A'].chatItems;
  ctx.state.messages = ctx.buffers['session-A'].messages;
  let resolveInvoke;
  const gate = new Promise(resolve => { resolveInvoke = resolve; });
  ctx.invoke = async () => { await gate; };
  const pending = webActions.cancelUserInput('wcard-A', 'call-A');
  ctx.state.activeSessionId = 'session-B';
  resolveInvoke();
  await pending;
  assert.ok(ctx.runOnSessionCalls.every(sid => sid === 'session-A'), 'web 取消定向到触发会话');
  assert.equal(ctx.buffers['session-A'].chatItems[0].cardState, 'cancelled', 'web 取消结果写回 A');
}

// ── Web 特殊 question id 提交：不触发原型链 TypeError ──────────────
{
  const ctx = makeWebContext({ activeSessionId: 'session-1' });
  Object.assign(webActions, ctx);
  for (const specialId of ['constructor', 'toString', '__proto__']) {
    ctx.state.chatItems = [{ id: 'wcard-s', type: 'user_input', toolCallId: 'call-s', questions: [{ id: specialId, header: '选择' }] }];
    await webActions.submitUserInput(
      'wcard-s',
      'call-s',
      [{ id: specialId, label: 'A', value: 'A' }],
      [{ id: specialId, header: '选择' }],
    );
    const card = ctx.state.chatItems[0];
    assert.equal(card.resolved, true, `web 特殊 id "${specialId}" 提交不得抛错`);
    assert.equal(card.cardState, 'submitted');
    assert.deepEqual(card.restoredAnswers, [{ id: specialId, label: 'A', value: 'A' }], `web restoredAnswers 写入特殊 id "${specialId}"`);
  }
}

for (const [label, bridgeSource] of [
  ['tauri bridge', read('src', 'platform', 'tauri', 'bridge.js')],
  ['web bridge', read('src', 'platform', 'web', 'bridge.js')],
]) {
  const parseUserAnswers = loadParseUserAnswers(bridgeSource);
  const questions = [
    { id: 'q1', header: '语言' },
    { id: 'q2', header: '技能' },
  ];
  // 多选：q2 两条答案，按 id 分组全量保留，顺序对齐 questions。
  const out = parseUserAnswers(JSON.stringify({ answers: [
    { id: 'q1', label: 'Python', value: 'Python' },
    { id: 'q2', label: '前端', value: '前端' },
    { id: 'q2', label: '运维', value: '运维' },
  ] }), questions);
  assert.deepEqual(out, [
    { id: 'q1', label: 'Python', value: 'Python' },
    { id: 'q2', label: '前端', value: '前端' },
    { id: 'q2', label: '运维', value: '运维' },
  ], `${label} parseUserAnswers 多选不塌缩`);
  // 未命中的问题占 null。
  assert.deepEqual(
    parseUserAnswers(JSON.stringify({ answers: [{ id: 'q1', label: 'Python', value: 'Python' }] }), questions),
    [{ id: 'q1', label: 'Python', value: 'Python' }, null],
    `${label} parseUserAnswers 未命中补 null`,
  );
  // 非法 JSON / 非数组 → null。
  assert.equal(parseUserAnswers('not json', questions), null, `${label} 非法 JSON 返回 null`);
  assert.equal(parseUserAnswers(JSON.stringify({ answers: 'x' }), questions), null, `${label} 非数组返回 null`);
  // 特殊 question id（constructor/toString/__proto__）不得命中 Object.prototype 继承属性。
  // 修复前 byId 是普通 {}，byId['constructor'] 取到 Object 构造器后 .push 抛 TypeError（复核 P1）。
  for (const specialId of ['constructor', 'toString', '__proto__']) {
    const special = parseUserAnswers(
      JSON.stringify({ answers: [{ id: specialId, label: 'A', value: 'A' }] }),
      [{ id: specialId, header: '选择' }],
    );
    assert.deepEqual(special, [{ id: specialId, label: 'A', value: 'A' }], `${label} parseUserAnswers 特殊 id "${specialId}" 不抛错且分组正常`);
  }
}

console.log('user_input_submit_logic: all assertions passed');
