#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(
  path.join(root, 'src', 'platform', 'tauri', 'bridge', 'chat-events.js'),
  'utf8',
);
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
  chatItems: [
    { id: 1, type: 'assistant', html: '', streaming: true },
  ],
  messages: [],
  thinking: { active: true },
};
const context = {
  state,
  listen(name, handler) { listeners.set(name, handler); },
  notify() {},
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
assert.ok(listeners.has('chat:reasoning_start'), 'the native bridge must listen for reasoning starts');
assert.ok(listeners.has('chat:reasoning_delta'), 'the native bridge must listen for reasoning deltas');
assert.ok(listeners.has('chat:reasoning_done'), 'the native bridge must listen for reasoning completion');

const emit = (name, payload) => listeners.get(name)({
  event: name,
  payload: { session_id: 'session-1', ...(payload || {}) },
});

emit('chat:reasoning_start', { index: 0 });
emit('chat:reasoning_delta', { index: 0, text: '先检查' });
emit('chat:reasoning_delta', { index: 0, text: '上下文' });
assert.deepEqual(
  state.chatItems.map(item => item.type),
  ['reasoning'],
  'the first reasoning chunk must replace the pre-created empty assistant bubble',
);
assert.equal(state.chatItems[0].text, '先检查上下文');
assert.equal(state.chatItems[0].streaming, true);
assert.deepEqual(JSON.parse(JSON.stringify(context.pendingAssistantBlocks)), [
  { type: 'thinking', thinking: '先检查上下文' },
]);

emit('chat:reasoning_done', { index: 0 });
assert.equal(state.chatItems[0].streaming, false);
assert.equal(typeof state.chatItems[0].completedAt, 'number');

emit('chat:reasoning_start', { index: 1 });
emit('chat:reasoning_delta', { index: 1, text: '相邻思考块' });
emit('chat:reasoning_done', { index: 1 });
assert.deepEqual(JSON.parse(JSON.stringify(context.pendingAssistantBlocks)), [
  { type: 'thinking', thinking: '先检查上下文' },
  { type: 'thinking', thinking: '相邻思考块' },
], 'explicit reasoning lifecycle must preserve adjacent thinking block boundaries');

emit('chat:delta', { text: '第一段回答' });
assert.deepEqual(state.chatItems.map(item => item.type), ['reasoning', 'reasoning', 'assistant']);
assert.equal(state.chatItems[2].html, '<p>第一段回答</p>');

emit('chat:reasoning_start', { index: 2 });
emit('chat:reasoning_delta', { index: 2, text: '重试前思考' });
emit('chat:transient_error', { error: 'SSE idle timeout' });
emit('chat:delta', { text: '最终结论' });
assert.deepEqual(
  state.chatItems.map(item => item.type),
  ['reasoning', 'reasoning', 'assistant', 'reasoning', 'system', 'assistant'],
  'a transient notice between reasoning and answer must preserve timeline order',
);
assert.equal(state.chatItems[2].streaming, false);
assert.equal(state.chatItems[3].text, '重试前思考');
assert.equal(state.chatItems[3].streaming, false, 'intervening notices must not leave stale reasoning active');
assert.deepEqual(JSON.parse(JSON.stringify(context.pendingAssistantBlocks)), [
  { type: 'thinking', thinking: '先检查上下文' },
  { type: 'thinking', thinking: '相邻思考块' },
  { type: 'text', text: '第一段回答' },
  { type: 'thinking', thinking: '重试前思考' },
]);
assert.equal(context.pendingAssistantText, '最终结论');

// Wave 2 把推理事件转发拆到 forwarder.rs；Event::Thinking* 与对应 emit 落在该子模块。
const forwarderSource = readFileSync(
  path.join(root, 'src-tauri', 'src', 'features', 'assistant', 'forwarder.rs'),
  'utf8',
);
assert.match(forwarderSource, /Event::ThinkingStarted\s*\{\s*index\s*\}/);
assert.match(forwarderSource, /app\.emit\("chat:reasoning_start"/);
assert.match(forwarderSource, /Event::ThinkingDelta\s*\{\s*index,\s*content\s*\}/);
assert.match(forwarderSource, /app\.emit\("chat:reasoning_delta"/);
assert.match(forwarderSource, /Event::ThinkingComplete\s*\{\s*index\s*\}/);
assert.match(forwarderSource, /app\.emit\("chat:reasoning_done"/);
assert.match(forwarderSource, /forward_app_event\([^;]*"chat:reasoning_delta"/s);

const webBridgeSource = readFileSync(
  path.join(root, 'src', 'platform', 'web', 'bridge.js'),
  'utf8',
);
assert.match(webBridgeSource, /listen\("chat:reasoning_start"/);
assert.match(webBridgeSource, /listen\("chat:reasoning_delta"/);
assert.match(webBridgeSource, /listen\("chat:reasoning_done"/);
assert.match(webBridgeSource, /item\.type === "reasoning"\) return "reasoning:" \+ String\(item\.text \|\| ""\)/);

const sessionBridgeSource = readFileSync(
  path.join(root, 'src', 'platform', 'tauri', 'bridge', 'sessions.js'),
  'utf8',
);
assert.match(sessionBridgeSource, /item\.type === "reasoning"\) return "reasoning:" \+ String\(item\.text \|\| ""\)/);

const chatViewSource = readFileSync(
  path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'),
  'utf8',
);
assert.match(
  chatViewSource,
  /if \(item\.type === 'reasoning'\) return undefined;/,
  'ChatView must delegate reasoning items to ConversationTimeline instead of the legacy ChatBubble',
);

console.log('model_reasoning_render: ok');
