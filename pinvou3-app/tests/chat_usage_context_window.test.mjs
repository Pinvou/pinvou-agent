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
const source = read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
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

// ── tauri/web 双端源码断言：窗口消费必须先于 dirty guard ──
const tauriSection = source.slice(source.indexOf('listen("chat:usage"'), source.indexOf('listen("chat:compaction"'));
const webSource = read('src', 'platform', 'web', 'bridge.js');
const webSection = webSource.slice(webSource.indexOf('listen("chat:usage"'), webSource.indexOf('listen("chat:compaction"'));

for (const [label, section] of [['tauri', tauriSection], ['web', webSection]]) {
  const dirtyGuard = section.indexOf('turnUsageDirty[sid]) return');
  const windowRead = section.indexOf('e.payload.context_window');
  assert.ok(dirtyGuard !== -1 && windowRead !== -1, `${label}: chat:usage handler 片段完整`);
  assert.ok(windowRead < dirtyGuard, `${label}: context_window 消费必须先于 dirty guard（否则工具轮永不更新分母）`);
  assert.ok(section.includes('windowTok !== state.tokens.max'), `${label}: 窗口变化才更新分母`);
  assert.ok(section.includes('windowTok > 0'), `${label}: 窗口 0 守卫保留`);
}

// ── Initial-state source assertions: neither bridge initializes with the
// 32K fake denominator (unknown window = 0; the UI hides the meter) ──
const tauriBridgeSource = read('src', 'platform', 'tauri', 'bridge.js');
const tauriMonitorSource = read('src', 'platform', 'tauri', 'bridge', 'monitor.js');
assert.ok(tauriBridgeSource.includes('tokens: { input: 0, max: 0 }'), 'tauri bridge initializes max=0 (unknown window)');
assert.ok(webSource.includes('tokens: { input: 0, max: 0 }'), 'web bridge initializes max=0 (unknown window)');
assert.ok(!webSource.includes('let maxModelLen = 32768'), 'web monitor must not fall back to a fake 32K window');
assert.ok(tauriMonitorSource.includes('state.tokens.max || 0'), 'tauri monitor must not fall back to a fake 32K window');

console.log('chat_usage_context_window.test.mjs: OK');
