#!/usr/bin/env node
// <context_update> 序号计数器（fork/rewind 入口表序号的序号前移来源）的生产链行为测试：
//   1. live 累加（chat:user_message 准入）→ 消费（随下一真实用户气泡下发）→ 去重（同事件
//      重复投递不得二次计数）→ chat:transcript_committed 的 context_updates_total 权威
//      重同步（live 通道从不播报引擎漂移增量，提交负载里的累计总数是唯一可信 live 来源）。
//   2. 形状钉：isContextUpdateRuntimeMessage 的 canonical 匹配（完整前缀+后缀、CRLF 归一、
//      提及不匹配）——与 Rust 侧 count_context_updates 同一 fixture 口径（engine.rs）。
//   3. 源码契约层：chat.js 发送失败返还待消费计数、chat-events.js 重同步接线、bridge.js
//      回放余量回种钩子保持在场。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => readFileSync(path.join(root, ...parts), 'utf8');

// ── 行为层：vm 驱动 tauri chat-events 监听器（复用 chat_manual_compact 装置形态）──
const source = read('src', 'shared', 'bridge-messages.js') + '\n' +
  read('src', 'shared', 'bridge-shared-helpers.js') + '\n' +
  read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
vm.runInContext(source, vm.createContext({
  window: windowObject,
  console,
  Date,
  String,
}), { filename: 'chat-events.js' });

const listeners = new Map();
const state = {
  activeSessionId: 'session-1',
  chatItems: [],
  messages: [],
  queued: [],
  thinking: { active: false },
  busy: false,
};
const buffers = {
  'session-1': { runtimeContextUpdatesPending: 0, remoteAdmissionKeys: [], messages: [], localTurnOwned: false },
  'session-2': { runtimeContextUpdatesPending: 0, remoteAdmissionKeys: [], messages: [], localTurnOwned: false },
};
const sessionStates = {
  // 后台会话已装载的工作集（真实 bridge 中 ensureSessionBufferLoaded 后才存在）
  'session-2': { chatItems: [{ type: 'assistant', text: 'bg reply' }] },
};
const context = {
  state,
  listen(name, handler) { listeners.set(name, handler); },
  notify() {},
  invoke: async () => null,
  turnUsageDirty: {},
  sessionStates,
  renderMarkdown(text) { return `<p>${text}</p>`; },
  bt() { return ''; },
  onSessionEvent(_event, callback) { callback(); },
  // 与真实 runSyncOnSession 同一契约：sid 会话的工作集换入换出（后台会话
  // 未装载时不执行——该状态下无从计算消费量，留给下次装载的回放重计）。
  runSyncOnSession(sessionId, callback) {
    if (!sessionId || sessionId === state.activeSessionId) { callback(); return; }
    const bg = sessionStates[sessionId];
    if (!bg) return;
    const saved = state.chatItems;
    state.chatItems = bg.chatItems;
    try { callback(); } finally { state.chatItems = saved; }
  },
  getBuffer(sid) { return buffers[sid] || null; },
  markRemoteTurn() {},
  isBusyFor() { return false; },
  reconcileRemoteTurn: async () => false,
  flushQueued() {},
  captureSteerPositions() {},
  steeredQueueFor() { return null; },
  recordAuthoritySyncDiagnostic() {},
  authoritySyncBufferSnapshot() { return {}; },
  ensureSessionBufferLoaded: async () => {},
  isInternalRuntimeUserMessage(value) {
    return windowObject.PinvouBridgeMessages.isInternalRuntimeUserMessage(value);
  },
  userMessageDisplayText(content) {
    return (content || []).map(b => (b && b.type === 'text' ? b.text : '')).join('');
  },
  addChatItem(item) {
    item.id = ++context.itemIdSeq;
    state.chatItems.push(item);
  },
  addSystemItem(text, meta = {}) {
    context.addChatItem({ type: 'system', text, ...meta });
  },
  timeStr() { return '12:00'; },
  startThinking() {},
  stopThinking() {},
  flushPendingStreamRender() {},
  flushPendingTextBlock() {},
  resetPendingAssistant() {},
  currentStreamId: 1,
  currentStreamText: '',
  pendingAssistantText: '',
  pendingAssistantBlocks: [],
  itemIdSeq: 1,
};
windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'](context);

const PinvouBridgeMessages = windowObject.PinvouBridgeMessages;
const buffer1 = buffers['session-1'];
const lastUserBubble = () => {
  for (let i = state.chatItems.length - 1; i >= 0; i--) {
    if (state.chatItems[i] && state.chatItems[i].type === 'user') return state.chatItems[i];
  }
  return null;
};
const emitUserMessage = payload => listeners.get('chat:user_message')({
  event: 'chat:user_message',
  id: payload.base_transcript_revision,
  payload: { session_id: 'session-1', ...payload },
});
const emitCommitted = payload => listeners.get('chat:transcript_committed')({
  event: 'chat:transcript_committed',
  payload: { session_id: 'session-1', ...payload },
});

const CANONICAL_UPDATE = [
  '<context_update>',
  'Session context changed since it was pinned; the pinned system prompt is unchanged. Delta (+ added, - removed):',
  '',
  '- stale line',
  '</context_update>',
].join('\n');

// ── 形状钉（与 Rust context_update_count_tests 同一 fixture 口径）──
assert.equal(PinvouBridgeMessages.isContextUpdateRuntimeMessage(CANONICAL_UPDATE), true, 'canonical 全形状必须匹配');
assert.equal(
  PinvouBridgeMessages.isContextUpdateRuntimeMessage(CANONICAL_UPDATE.replace('\n</context_update>', '')),
  false,
  '缺闭合标签的截断片段不得匹配',
);
assert.equal(
  PinvouBridgeMessages.isContextUpdateRuntimeMessage('quoted mention: <context_update> not canonical'),
  false,
  '用户提及/引用不得匹配',
);
assert.equal(
  PinvouBridgeMessages.isContextUpdateRuntimeMessage(CANONICAL_UPDATE.replaceAll('\n', '\r\n')),
  true,
  'CRLF 必须归一后匹配',
);

// ── 场景 1：live 准入对 canonical 形状累加，且不渲染气泡 ──
emitUserMessage({ content: CANONICAL_UPDATE, base_transcript_revision: 'rev-1' });
assert.equal(buffer1.runtimeContextUpdatesPending, 1, 'canonical 准入必须累加待消费计数');
assert.equal(state.chatItems.filter(item => item.type === 'user').length, 0, '运行时消息不得产生用户气泡');

// ── 场景 2：消费——下一个真实准入的气泡携带计数并清零 ──
emitUserMessage({ content: 'second prompt', base_transcript_revision: 'rev-2' });
const bubble = lastUserBubble();
assert.ok(bubble, '真实准入必须产生用户气泡');
assert.equal(bubble.runtimeContextUpdatesBefore, 1, '气泡必须携带此前累加的边界计数');
assert.equal(buffer1.runtimeContextUpdatesPending, 0, '随气泡下发后计数清零');

// ── 场景 3：重复投递不得二次计数（同 event id 与同 base revision 两条去重路径）──
emitUserMessage({ content: 'second prompt', base_transcript_revision: 'rev-2' });
assert.equal(buffer1.runtimeContextUpdatesPending, 0, '重复投递（同 admission key）不得二次累加');
const userBubbleCount = state.chatItems.filter(item => item.type === 'user').length;
emitUserMessage({ content: CANONICAL_UPDATE, base_transcript_revision: 'rev-2' });
assert.equal(buffer1.runtimeContextUpdatesPending, 0, '重复事件即便形状是 canonical 也不得累加');
assert.equal(state.chatItems.filter(item => item.type === 'user').length, userBubbleCount, '重复事件不得产生新气泡');

// ── 场景 4：提交重同步——live 通道看不到的漂移增量由总数收敛 ──
// 后端此刻转录里有 3 个 <context_update>（live 只数到过 1 个）。
emitCommitted({ transcript_revision: 'rev-3', context_updates_total: 3 });
assert.equal(buffer1.runtimeContextUpdatesPending, 2, '重同步 pending = 总数 − 已消费(1)');

// ── 场景 5：重同步后的计数照常随下一气泡消费 ──
emitUserMessage({ content: 'third prompt', base_transcript_revision: 'rev-4' });
assert.equal(lastUserBubble().runtimeContextUpdatesBefore, 2, '重同步后的余量随下一气泡下发');
assert.equal(buffer1.runtimeContextUpdatesPending, 0, '下发后清零');

// ── 场景 6：总数低于已消费时钳制在 0（回滚/编辑场景不得出现负数）──
emitCommitted({ transcript_revision: 'rev-5', context_updates_total: 1 });
assert.equal(buffer1.runtimeContextUpdatesPending, 0, 'pending 钳制在 0');

// ── 场景 7：重同步按 sid 路由，不得污染活动会话 ──
listeners.get('chat:transcript_committed')({
  event: 'chat:transcript_committed',
  payload: { session_id: 'session-2', transcript_revision: 'rev-9', context_updates_total: 4 },
});
assert.equal(buffers['session-2'].runtimeContextUpdatesPending, 4, '后台会话按自身 sid 重同步');
assert.equal(buffer1.runtimeContextUpdatesPending, 0, '活动会话计数不受其他会话提交影响');

// ── 场景 8：无总数负载（旧后端）不得改动计数 ──
emitCommitted({ transcript_revision: 'rev-6' });
assert.equal(buffer1.runtimeContextUpdatesPending, 0, '缺 context_updates_total 时保持原值');

// ── 纯函数单元：消费合计与钳制 ──
const fakeItems = [
  { type: 'user', runtimeContextUpdatesBefore: 2 },
  { type: 'assistant', text: 'reply' },
  { type: 'user', runtimeContextUpdatesBefore: 3 },
  { type: 'user' },
];
assert.equal(PinvouBridgeMessages.runtimeContextUpdatesConsumedTotal(fakeItems), 5, '消费合计只算用户气泡');
const resyncBuffer = { runtimeContextUpdatesPending: 99 };
PinvouBridgeMessages.resyncRuntimeContextUpdatesPending(resyncBuffer, fakeItems, 4);
assert.equal(resyncBuffer.runtimeContextUpdatesPending, 0, '总数低于消费时钳制');
PinvouBridgeMessages.resyncRuntimeContextUpdatesPending(resyncBuffer, fakeItems, 9);
assert.equal(resyncBuffer.runtimeContextUpdatesPending, 4, 'pending = 总数 − 消费');
PinvouBridgeMessages.resyncRuntimeContextUpdatesPending(null, fakeItems, 9);
PinvouBridgeMessages.resyncRuntimeContextUpdatesPending(resyncBuffer, undefined, 2);
assert.equal(resyncBuffer.runtimeContextUpdatesPending, 2, '空 chatItems 视为零消费');

// ── 源码契约层 ──
const chatJs = read('src', 'platform', 'tauri', 'bridge', 'chat.js');
assert.ok(
  chatJs.includes('(turnOwnerBuffer.runtimeContextUpdatesPending || 0) + submittedContextUpdates'),
  '发送失败必须把未下发的边界计数返还 pending（chat.js catch 路径）',
);
const chatEventsJs = read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
assert.ok(
  chatEventsJs.includes('resyncRuntimeContextUpdatesPending') &&
  chatEventsJs.includes('payload.context_updates_total'),
  'transcript_committed 监听器必须接线 context_updates_total 重同步',
);
assert.ok(
  chatEventsJs.indexOf('remoteAdmissionKeys.includes(admissionKey)') <
  chatEventsJs.indexOf('runtimeContextUpdatesPending = (userBuffer.runtimeContextUpdatesPending || 0) + 1'),
  '累加必须在 admission 去重之后（重复投递不得二次计数）',
);
const bridgeJs = read('src', 'platform', 'tauri', 'bridge.js');
assert.ok(
  bridgeJs.includes('setRuntimeContextUpdatesPending'),
  '回放余量回种钩子必须保持在场（bridge.js lane hook）',
);

console.log('context_update_counter: all assertions passed');
