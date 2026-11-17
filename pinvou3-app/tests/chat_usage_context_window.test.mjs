#!/usr/bin/env node
// chat:usage 的 context_window 消费时序回归：dirty 工具轮（turnUsageDirty=true）
// 只跳过不可信的累计 input_tokens，仍必须消费真实窗口分母（PR #213 核心目标）。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => readFileSync(path.join(root, ...parts), 'utf8');

// ── tauri 端行为测试：dirty 工具轮仍更新 context window、但不更新 input ──
const source = read('src', 'shared', 'bridge-shared-helpers.js') + '\n' + read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
vm.runInContext(source, vm.createContext({
  window: windowObject,
  console,
  Date,
  String,
}), { filename: 'chat-events.js' });

const listeners = new Map();
let notifyCount = 0;
const state = {
  activeSessionId: 'session-1',
  chatItems: [],
  messages: [],
  tokens: { input: 0, max: 32768 }, // 种子值：模拟「窗口已知」的会话（历史版本曾以 32K 假分母起步）
  thinking: { active: false },
};
const context = {
  state,
  listen(name, handler) { listeners.set(name, handler); },
  notify() { notifyCount += 1; },
  invoke: async () => null,
  turnUsageDirty: {},
  sessionStates: {},
  renderMarkdown(text) { return `<p>${text}</p>`; },
  bt() { return ''; },
  onSessionEvent(_event, callback) { callback(); },
  runSyncOnSession(_sessionId, callback) { callback(); },
  addChatItem(item) {
    item.id = ++context.itemIdSeq;
    state.chatItems.push(item);
  },
  addSystemItem(text, meta = {}) {
    context.addChatItem({ type: 'system', text, ...meta });
  },
  timeStr() { return '12:00'; },
  flushPendingTextBlock() {
    if (!context.pendingAssistantText) return;
    context.pendingAssistantBlocks.push({ type: 'text', text: context.pendingAssistantText });
    context.pendingAssistantText = '';
  },
  currentStreamId: 1,
  currentStreamText: '',
  pendingAssistantText: '',
  pendingAssistantBlocks: [],
  itemIdSeq: 1,
};
windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'](context);

const emitUsage = payload => listeners.get('chat:usage')({
  event: 'chat:usage',
  payload: { session_id: 'session-1', ...payload },
});

// 场景 1：dirty 工具轮（tool_start 已置 dirty）——分母必须更新，input 保留。
// 这是最常见 Agent 场景：PR #213 修复前此处提前 return，真实窗口永不消费。
context.turnUsageDirty['session-1'] = true;
emitUsage({ input_tokens: 45000, context_window: 131072 });
assert.equal(state.tokens.max, 131072, 'dirty 工具轮仍必须消费真实窗口');
assert.equal(state.tokens.input, 0, 'dirty 工具轮不得更新累计 input（本轮累加值不可信）');
assert.ok(notifyCount >= 1, '窗口变化时必须通知 UI 刷新分母');

// 场景 2：同一窗口再次到达——不重复 notify，但 input 仍跳过（仍是 dirty 轮）。
const notifyBefore = notifyCount;
emitUsage({ input_tokens: 45001, context_window: 131072 });
assert.equal(state.tokens.max, 131072);
assert.equal(state.tokens.input, 0, 'dirty 轮 input 保持冻结');
assert.equal(notifyCount, notifyBefore, '窗口未变化不重复通知');

// 场景 3：干净轮（新一轮开始，dirty 已重置）——input 恢复更新。
context.turnUsageDirty['session-1'] = false;
emitUsage({ input_tokens: 2048, context_window: 131072 });
assert.equal(state.tokens.max, 131072);
assert.equal(state.tokens.input, 2048, '干净轮 input 正常更新');
assert.ok(notifyCount > notifyBefore, 'input 更新触发通知');

// 场景 4：超窗累加值（内部重试等无事件轮）——跳过显示超上限数字。
context.turnUsageDirty['session-1'] = false;
const inputBefore = state.tokens.input;
emitUsage({ input_tokens: 200000, context_window: 131072 });
assert.equal(state.tokens.input, inputBefore, '超过窗口的累加值不得显示（保留上次准确值）');
assert.equal(state.tokens.max, 131072, '超窗轮仍保留真实分母');

// 场景 5：无 context_window 的旧远端（降级）——保留旧 max，不破坏。
const maxBefore = state.tokens.max;
emitUsage({ input_tokens: 100 }); // 无 context_window 字段
assert.equal(state.tokens.max, maxBefore, '旧端不带 context_window 时保留旧值');
assert.equal(state.tokens.input, 100);

// Scenario 6: unknown-window start (bridge initializes max=0, no more 32K
// fake denominator) — a usage without context_window must not introduce a
// fake denominator, nor surface input without denominator backing.
state.tokens = { input: 0, max: 0 };
context.turnUsageDirty['session-1'] = false;
emitUsage({ input_tokens: 100 }); // 旧端不带 context_window
assert.equal(state.tokens.max, 0, 'unknown window stays 0; no fake denominator may be introduced');
assert.equal(state.tokens.input, 0, 'input must not surface without a real denominator (100 > 0=max is skipped by the guard)');

// Scenario 7: a session that started with an unknown window later receives
// a real one → the denominator lands and input is consumed normally.
emitUsage({ input_tokens: 5000, context_window: 262144 });
assert.equal(state.tokens.max, 262144, 'denominator lands once the real window arrives');
assert.equal(state.tokens.input, 5000);

// 场景 8（本次修复的时序回归）：dirty 工具轮 + 逐请求占用口径。chat:usage
// 每轮只在 TurnComplete 发一次，而本轮任意 tool_start 会把 dirty 锁到下一轮
// 发送——工具轮此前只能靠压缩 post_tokens 刷新，读数永远冻结在「— / … · 0%」
// （issue: 1.0.1 上下文长度必现空白）。逐请求值（引擎最后一次父请求的
// reported input = 回合结束时的真实占用）不是累加值，必须绕过 dirty guard。
context.turnUsageDirty['session-1'] = true;
emitUsage({ input_tokens: 45000, context_window: 262144, context_input_tokens: 61000 });
assert.equal(state.tokens.max, 262144);
assert.equal(state.tokens.input, 61000, 'dirty 工具轮必须消费逐请求占用（只吃累计口径是冻结根因）');

// 场景 9：干净轮同时携带两种口径——逐请求值胜出，累计口径不得覆写。
context.turnUsageDirty['session-1'] = false;
emitUsage({ input_tokens: 90000, context_window: 262144, context_input_tokens: 62000 });
assert.equal(state.tokens.input, 62000, '逐请求口径消费后同事件累计口径不得覆写');

// 场景 10：逐请求值超窗（分母声明失真）——丢弃且不回落累计口径，保留上个准确值。
const goodInput = state.tokens.input;
emitUsage({ input_tokens: 45000, context_window: 262144, context_input_tokens: 999999 });
assert.equal(state.tokens.input, goodInput, '超窗的逐请求值不得显示，也不得回落累计口径');

// 场景 11：引擎未上报 usage（context_input_tokens 为 null）+ dirty 轮——
// 回落旧口径：dirty 轮仍跳过累计值，读数保留上个准确值（行为不变）。
context.turnUsageDirty['session-1'] = true;
emitUsage({ input_tokens: 45000, context_window: 262144, context_input_tokens: null });
assert.equal(state.tokens.input, goodInput, '未上报占用时 dirty 轮仍跳过累计口径');

// 场景 12：未上报占用 + 干净轮——累计口径兜底（旧引擎/降级远端路径不变）。
context.turnUsageDirty['session-1'] = false;
emitUsage({ input_tokens: 63000, context_window: 262144 });
assert.equal(state.tokens.input, 63000, '无逐请求值时累计口径兜底不变');

// ── tauri/web 双端源码断言：窗口与逐请求占用都必须先于 dirty guard 消费 ──
const tauriSection = source.slice(source.indexOf('listen("chat:usage"'), source.indexOf('listen("chat:compaction"'));
const webSource = read('src', 'platform', 'web', 'bridge.js');
const webSection = webSource.slice(webSource.indexOf('listen("chat:usage"'), webSource.indexOf('listen("chat:compaction"'));

for (const [label, section] of [['tauri', tauriSection], ['web', webSection]]) {
  const dirtyGuard = section.indexOf('turnUsageDirty[sid]) return');
  const windowRead = section.indexOf('e.payload.context_window');
  const ctxInputRead = section.indexOf('e.payload.context_input_tokens');
  assert.ok(dirtyGuard !== -1 && windowRead !== -1, `${label}: chat:usage handler 片段完整`);
  assert.ok(windowRead < dirtyGuard, `${label}: context_window 消费必须先于 dirty guard（否则工具轮永不更新分母）`);
  assert.ok(ctxInputRead !== -1, `${label}: 逐请求占用口径消费点缺失`);
  assert.ok(ctxInputRead < dirtyGuard, `${label}: 逐请求占用必须先于 dirty guard 消费（工具轮读数冻结根因）`);
  assert.ok(windowRead < ctxInputRead, `${label}: 分母更新必须先于逐请求占用消费（同一事件先立分母）`);
  assert.ok(section.includes('windowTok !== state.tokens.max'), `${label}: 窗口变化才更新分母`);
  assert.ok(section.includes('windowTok > 0'), `${label}: 窗口 0 守卫保留`);
}

// ── Initial-state source assertions: neither bridge initializes with the
// 32K fake denominator (unknown window = 0; the UI hides the meter) ──
const tauriBridgeSource = read('src', 'platform', 'tauri', 'bridge.js');
assert.ok(tauriBridgeSource.includes('tokens: { input: 0, max: 0 }'), 'tauri bridge initializes max=0 (unknown window)');
assert.ok(webSource.includes('tokens: { input: 0, max: 0 }'), 'web bridge initializes max=0 (unknown window)');
assert.ok(!webSource.includes('let maxModelLen = 32768'), 'web monitor must not fall back to a fake 32K window');
// Wave 1 moved pollMonitor into the shared base; pin the shared body.
assert.ok((read('src', 'shared', 'bridge-shared-helpers.js')).includes('state.tokens.max = snap.vllm.max_model_len;'), 'tauri monitor must not fall back to a fake 32K window');

console.log('chat_usage_context_window.test.mjs: OK');
