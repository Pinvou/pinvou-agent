#!/usr/bin/env node
// 主车道手动压缩入口（ctx 用量行按钮）的契约测试：
//   1. 行为层（vm 驱动 tauri chat-events 监听器）：chat:compaction start→终态
//      维护 state.compactingSessions 在途标记；done 的 post_tokens 是压缩后
//      准确占用（forwarder 为刷新用量条而加），先于 dirty guard 消费。
//   2. 源码契约层：ChatView 按钮守卫（busy/compacting/按会话入队）、
//      session_engine_not_running 本地化映射、web 双端同改、i18n 每语言恰一份。
// 复用 chat_usage_context_window.test.mjs 的 vm 装置。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => readFileSync(path.join(root, ...parts), 'utf8');

// ── tauri 端行为测试：compactingSessions 标记 + post_tokens 消费 ──
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
const pruneMerges = [];
const state = {
  activeSessionId: 'session-1',
  chatItems: [],
  messages: [],
  tokens: { input: 40000, max: 131072 },
  thinking: { active: false },
  compactingSessions: {},
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
  addOrMergePruneCompaction(compactId) { pruneMerges.push(compactId); },
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

const emitCompaction = payload => listeners.get('chat:compaction')({
  event: 'chat:compaction',
  payload: { session_id: 'session-1', ...payload },
});

// 场景 1：start 置位在途标记（手动与自动压缩共用同一事件流）。
emitCompaction({ phase: 'start', id: 'compact-1', auto: false, message: 'Manual context compaction started' });
assert.equal(state.compactingSessions['session-1'], true, 'start 必须置位压缩在途标记');
assert.equal(state.chatItems.length, 1, 'start 系统项照常入列');

// 场景 2：done 清除标记，且 post_tokens 是压缩后的准确占用——直接刷新
// 用量条（保留真实窗口分母），不等下一轮 usage。
const notifyAfterStart = notifyCount;
emitCompaction({ phase: 'done', id: 'compact-1', messages_before: 12, messages_after: 8, post_tokens: 9500, message: '12 → 8 messages' });
assert.equal(state.compactingSessions['session-1'], undefined, 'done 终态必须清除在途标记');
assert.equal(state.tokens.input, 9500, 'post_tokens 必须刷新用量条 input');
assert.equal(state.tokens.max, 131072, '真实窗口分母保持不变');
assert.ok(notifyCount > notifyAfterStart, '标记清除/用量刷新必须通知 UI');

// 场景 3：post_tokens 守卫——缺失/非正数/超窗不得污染用量条（保留上次准确值）。
for (const bad of [undefined, 0, -5, 999999]) {
  emitCompaction({ phase: 'start' });
  const inputBefore = state.tokens.input;
  emitCompaction({ phase: 'done', messages_before: 8, messages_after: 8, post_tokens: bad, message: 'x' });
  assert.equal(state.tokens.input, inputBefore, `post_tokens=${bad} 不得更新用量条`);
}

// 场景 4：fail/cancel 终态同样清除标记——不能留下永久禁用的入口。
for (const phase of ['fail', 'cancel']) {
  emitCompaction({ phase: 'start' });
  assert.equal(state.compactingSessions['session-1'], true);
  emitCompaction({ phase, message: 'x' });
  assert.equal(state.compactingSessions['session-1'], undefined, `${phase} 终态必须清除在途标记`);
}

// 场景 5：prune-only 自动压缩（done+pruneOnlyAuto 合并路径）也要清标记。
emitCompaction({ phase: 'start' });
emitCompaction({ phase: 'done', auto: true, messages_before: 8, messages_after: 8, message: '2 tool results pruned' });
assert.equal(state.compactingSessions['session-1'], undefined, 'prune-only 合并路径同样必须清标记');
assert.equal(pruneMerges.length, 1, 'prune-only 合并路径不受影响');

// 场景 6：无 session_id 的畸形事件——不得写标记，也不得炸监听器。
assert.doesNotThrow(() => emitCompaction({ phase: 'start', session_id: undefined }));
assert.equal(Object.keys(state.compactingSessions).length, 0, '无 session_id 不得写标记');

// 场景 7：压缩轮的 usage 事件仍被 dirty guard 冻结（post_tokens 已给出准确值，
// 后续累计 input 不可信——语义与 chat_usage_context_window 的 dirty 场景一致）。
const emitUsage = payload => listeners.get('chat:usage')({
  event: 'chat:usage',
  payload: { session_id: 'session-1', ...payload },
});
context.turnUsageDirty['session-1'] = true;
emitUsage({ input_tokens: 40000 });
assert.equal(state.tokens.input, 9500, '压缩轮后续累计 input 不可信，保留 post_tokens 值');

// ── web 双端源码契约：压缩 handler 必须同步维护标记并消费 post_tokens ──
const tauriCompaction = source.slice(source.indexOf('listen("chat:compaction"'));
const webSource = read('src', 'platform', 'web', 'bridge.js');
const webCompaction = webSource.slice(webSource.indexOf('listen("chat:compaction"'));
for (const [label, section] of [['tauri', tauriCompaction], ['web', webCompaction]]) {
  assert.ok(section.includes('state.compactingSessions[sid] = true'), `${label}: start 必须置位标记`);
  assert.ok(section.includes('delete state.compactingSessions[sid]'), `${label}: 终态必须清标记`);
  assert.ok(section.includes('post_tokens'), `${label}: 必须消费 post_tokens 刷新用量条`);
  assert.ok(section.includes('postTokens <= state.tokens.max'), `${label}: post_tokens 超窗守卫保留`);
}

// ── 桥接状态接线：字段进切片、两端 state 声明、删除/驱逐同清 ──
const sharedSource = read('src', 'shared', 'bridge-shared-helpers.js');
assert.ok(sharedSource.includes('"compactingSessions"'), 'STATE_SLICE_FIELDS.chat 必须声明 compactingSessions（两 lane 快照共用同一常量）');
const tauriBridgeSource = read('src', 'platform', 'tauri', 'bridge.js');
assert.equal(tauriBridgeSource.split('compactingSessions: {}').length - 1, 1, 'tauri state 必须声明 compactingSessions');
assert.equal(webSource.split('compactingSessions: {}').length - 1, 1, 'web state 必须声明 compactingSessions');
const tauriSessionsSource = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
assert.equal(tauriSessionsSource.split('delete state.compactingSessions[id]').length - 1, 3, 'tauri 三处删除/驱逐点必须同清标记');
assert.equal(webSource.split('delete state.compactingSessions[id]').length - 1, 3, 'web 三处删除/驱逐点必须同清标记');

// ── ChatView 源码契约：守卫、按会话入队、错误映射、可测性挂钩 ──
const chatViewSource = read('src', 'features', 'chat', 'ChatView.jsx').replace(/\r\n/g, '\n');
// 守卫必须同时挡 busy（轮中入队→轮末前无反馈）与 compactingActive（终态前重复入队）。
assert.match(chatViewSource, /if \(!sid \|\| manualCompactingSessionId === sid \|\| compactingActive \|\| busy\) return;/,
  '入口守卫必须同时挡 busy 与压缩在途，否则轮中点击是静默空操作且按钮复位后重复入队');
// 命令只入队即返回：sid 必须在调用点捕获（切会话后错误仍落到原会话）。
assert.match(chatViewSource, /const sid = activeSessionId;\s*if \(!sid[\s\S]*?setManualCompactingSessionId\(sid\);/, 'sid 必须在 invoke 前捕获');
assert.match(chatViewSource, /invokeTauri\('compact_now', \{ sessionId: sid \}\)/, '压缩命令与参数形状');
// 按会话 keying（ChatView 切会话不 remount）：布尔会在 A 入队中误禁 B。
assert.match(chatViewSource, /useState\(null\);\s*const compactingActive = !!\(activeSessionId && bs && bs\.compactingSessions && bs\.compactingSessions\[activeSessionId\]\);/s,
  '手动入队状态必须按会话记录');
assert.match(chatViewSource, /setManualCompactingSessionId\(current => \(current === sid \? null : current\)\);/, '复位必须校验会话身份');
// 引擎懒启动错误映射：裸错误链不得直接露出（与代码车道 nativeCompactInactive 同款）。
assert.match(chatViewSource, /raw\.includes\('session_engine_not_running'\) \? t\.uiCodex\.nativeCompactInactive : raw/, '引擎未运行必须映射本地化文案');
assert.ok(!chatViewSource.includes('compactFailNotice'), '失败文案必须复用 uiCodex.compactFail，不得新开重复 key');
// UI 挂钩与禁用形状。
assert.match(chatViewSource, /data-testid="chat-ctx-compact"/, '按钮必须带 testid（smoke 可达）');
assert.match(chatViewSource, /disabled=\{busy \|\| ctxCompactInFlight\}/, '禁用必须含 busy 与压缩在途');

// ── i18n：每语言恰一份（count-1 per locale），重复 key 不得回流 ──
for (const locale of ['en', 'zh', 'ja']) {
  const dictSource = read('src', 'shared', 'i18n', `${locale}.js`);
  for (const key of ['compactNowTip', 'compactingNow']) {
    assert.equal(dictSource.split(`${key}:`).length - 1, 1, `${locale}: ${key} 必须恰一份`);
  }
  assert.ok(!dictSource.includes('compactFailNotice'), `${locale}: 不得与 uiCodex.compactFail 重复新开 key`);
}
