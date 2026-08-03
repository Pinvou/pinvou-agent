import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

const chatSource = read('src', 'platform', 'tauri', 'bridge', 'chat.js');
const chatEventsSource = read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
const desktopBridgeSource = read('src', 'platform', 'tauri', 'bridge.js');
const webBridgeSource = read('src', 'platform', 'web', 'bridge.js');
const webTurnTerminalSource = read('src', 'platform', 'web', 'bridge', 'turn-terminal.js');
const chatViewSource = read('src', 'features', 'chat', 'ChatView.jsx');
const bridgeMessagesSource = read('src', 'shared', 'bridge-messages.js');
const { conversationItemsForMode } = await import(
  '../src/features/conversation/deepseek-conversation.js'
);

const sandbox = { window: {} };
vm.runInNewContext(chatSource, sandbox, { filename: 'chat.js' });
const installChat = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;

const messageSandbox = { window: {} };
vm.runInNewContext(bridgeMessagesSource, messageSandbox, { filename: 'bridge-messages.js' });
const cleanupState = { settings: { language: 'ja' }, chatItems: [] };
const addCleanupItem = (text, metadata) => cleanupState.chatItems.push({ text, ...metadata });
messageSandbox.window.PinvouBridgeMessages.showShellCleanupFailure(
  { shell_cleanup_failed: true },
  cleanupState,
  addCleanupItem,
);
assert.equal(cleanupState.chatItems.length, 1);
assert.match(cleanupState.chatItems[0].text, /バックグラウンドタスク/);
messageSandbox.window.PinvouBridgeMessages.showShellCleanupFailure(
  { shell_cleanup_failed: true },
  cleanupState,
  addCleanupItem,
);
assert.equal(cleanupState.chatItems.length, 1, 'cleanup warning must be deduplicated');
assert.equal(cleanupState.chatItems[0].legacyConversationOnly, true);

const terminalSandbox = { window: {}, Date };
vm.runInNewContext(webTurnTerminalSource, terminalSandbox, { filename: 'turn-terminal.js' });
const timelineState = {
  activeTurnTimelineId: 'turn-1',
  turnTimeline: [{ turn_id: 'turn-1', event: 'user_start', ui_turn_index: 2 }],
};
terminalSandbox.window.PinvouWebTurnTerminal.recordCompleted(
  timelineState,
  timelineState.turnTimeline[0],
  { status: 'Interrupted' },
);
assert.equal(timelineState.activeTurnTimelineId, null);
assert.equal(timelineState.turnTimeline[1].event, 'assistant_done');
assert.equal(timelineState.turnTimeline[1].status, 'Interrupted');
assert.equal(timelineState.turnTimeline[1].ui_turn_index, 2);

const state = {
  activeSessionId: 'session-1',
  chatItems: [
    { id: 1, type: 'system', text: '⚠️ 上一轮模型不可用', turnErrorNotice: true },
    { id: 2, type: 'system', text: '保留的会话通知' },
  ],
  messages: [],
  busy: false,
};
const buffer = {
  localTurnOwned: false,
  remoteTurnActive: false,
  remoteTerminalSeen: false,
  deferredRemoteUserEvent: null,
};
let rejectChat = true;
const context = {
  state,
  invoke(command) {
    if (command === 'remote_control_publish_user_message') return Promise.resolve();
    return rejectChat ? Promise.reject(new Error('当前模型不可用')) : Promise.resolve();
  },
  notify() {},
  TAURI: null,
  sessionStates: { 'session-1': buffer },
  turnUsageDirty: {},
  personaPlaceholderTitles: {},
  renderMarkdown(value) { return value; },
  safeConsoleInfo() {},
  bt(key) { return key; },
  runSyncOnSession(_sid, action) { action(); },
  startThinking() {},
  stopThinking() {},
  ensureSessionBufferLoaded() { return Promise.resolve(); },
  ensureSession() { return Promise.resolve('session-1'); },
  getBuffer() { return buffer; },
  reconcileRemoteTurn() { return Promise.resolve(true); },
  markRemoteTurn() {},
  clearAttachments() {},
  isScheduledRunSession() { return false; },
  basename(value) { return path.basename(String(value || '')); },
  extractArtifactPath() { return ''; },
  parseScheduledTaskDraftFromText() { return null; },
  autoCreateScheduledTaskDraft() {},
  pendingAssistantText: '',
  pendingAssistantBlocks: [],
  currentStreamText: '',
  currentStreamId: 0,
  itemIdSeq: 10,
};
const chat = installChat(context);

await chat.doSendFor('session-1', '第一次', '第一次', [], null, false, false);
assert.equal(
  state.chatItems.filter(item => item.turnErrorNotice).length,
  1,
  '发送失败时只保留当前一次临时错误',
);
assert.match(state.chatItems.find(item => item.turnErrorNotice).text, /当前模型不可用/);
assert.ok(state.chatItems.some(item => item.text === '保留的会话通知'));

rejectChat = false;
await chat.doSendFor('session-1', '重试', '重试', [], null, false, false);
assert.equal(
  state.chatItems.some(item => item.turnErrorNotice),
  false,
  '下一轮开始时必须清除上一轮临时错误',
);
assert.ok(state.chatItems.some(item => item.type === 'user' && item.text === '重试'));

const legacyFinalError = {
  id: 3,
  type: 'system',
  text: '⚠️ 最终模型错误',
  turnErrorNotice: true,
  legacyConversationOnly: true,
};
assert.deepEqual(
  conversationItemsForMode([legacyFinalError], false),
  [legacyFinalError],
  '旧版会话界面必须继续显示最终错误',
);
assert.deepEqual(
  conversationItemsForMode([legacyFinalError], true),
  [],
  '新版时间线已呈现最终错误，不应重复投影兼容气泡',
);

const doneSection = chatEventsSource.slice(
  chatEventsSource.indexOf('listen("chat:done"'),
  chatEventsSource.indexOf('listen("chat:usage"'),
);
assert.match(doneSection, /legacyConversationOnly: true/);
assert.match(bridgeMessagesSource, /payload\.shell_cleanup_failed/);
assert.match(doneSection, /PinvouBridgeMessages\.showShellCleanupFailure/);
assert.match(chatEventsSource, /turnErrorNotice && item\.text === notice/);
assert.match(chatEventsSource, /addSystemItem\(notice, \{ turnErrorNotice: true \}\)/);
assert.match(
  desktopBridgeSource,
  /if \(item\.turnErrorNotice && !item\.legacyConversationOnly\) return false/,
);
assert.match(chatViewSource, /conversationItemsForMode\(visibleChatItems, useUnifiedConversationUi\)/);

assert.match(webBridgeSource, /turnErrorNotice && item\.text === notice/);
assert.match(
  webBridgeSource,
  /if \(item\.turnErrorNotice && !item\.legacyConversationOnly\) return false/,
);
assert.match(
  webBridgeSource.slice(
    webBridgeSource.indexOf('listen("chat:done"'),
    webBridgeSource.indexOf('listen("chat:usage"'),
  ),
  /legacyConversationOnly: true/,
);
assert.match(bridgeMessagesSource, /payload\.shell_cleanup_failed/);
assert.match(webBridgeSource, /PinvouBridgeMessages\.showShellCleanupFailure/);
assert.equal(
  (bridgeMessagesSource.match(/^    (zh|en|ja):/gm) || []).length,
  3,
  'Shell cleanup warning must provide zh/en/ja translations',
);

console.log('chat turn error isolation: ok');
