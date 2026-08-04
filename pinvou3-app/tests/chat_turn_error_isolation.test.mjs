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
const modelServiceErrorsSource = read('src', 'shared', 'model-service-errors.js');
const bridgeMessagesSource = read('src', 'shared', 'bridge-messages.js');
const { conversationItemsForMode } = await import(
  '../src/features/conversation/deepseek-conversation.js'
);

const sandbox = { window: {} };
vm.runInNewContext(chatSource, sandbox, { filename: 'chat.js' });
const installChat = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;

const messageSandbox = { window: {} };
vm.runInNewContext(modelServiceErrorsSource, messageSandbox, { filename: 'model-service-errors.js' });
vm.runInNewContext(bridgeMessagesSource, messageSandbox, { filename: 'bridge-messages.js' });
const modelErrors = messageSandbox.window.PinvouModelServiceErrors;
assert.equal(modelErrors.classify('SSE stream request failed: HTTP 402 insufficient balance').kind, 'billing');
assert.equal(modelErrors.classify('HTTP 429 quota exceeded').kind, 'quota');
assert.equal(modelErrors.classify('HTTP 429 insufficient_quota').kind, 'quota');
assert.equal(modelErrors.classify('insufficient_quota').kind, 'quota');
assert.equal(modelErrors.classify('HTTP 429 too many requests').kind, 'rate_limit');
assert.equal(modelErrors.classify('HTTP 500 insufficient balance').kind, 'billing');
assert.equal(modelErrors.classify('permission denied while reading local file').kind, 'unknown');
assert.equal(modelErrors.isModelServiceError('permission denied while reading local file'), false);
assert.equal(modelErrors.isModelServiceError('insufficient_quota'), true);
assert.match(
  modelErrors.redactTechnicalDetail('Authorization: Bearer sk-deepseek-secret-token-123 api_key=sk-abc12345&token=demo'),
  /\[敏感信息已隐藏\]/,
);
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

const modelErrorState = {
  settings: { language: 'zh-Hans' },
  currentSessionModelId: 'deepseek-main',
  savedModels: [{ id: 'deepseek-main', preset: 'deepseek', model: 'deepseek-chat' }],
  chatItems: [],
};
const addModelErrorItem = (text, metadata) => modelErrorState.chatItems.push({ text, ...metadata });
const rawBillingError = 'SSE stream request failed: HTTP 402 {"error":{"message":"insufficient balance","api_key":"sk-secret"}}';
const billingAdded = messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: rawBillingError },
  modelErrorState,
  addModelErrorItem,
  true,
);
assert.equal(billingAdded, true);
assert.equal(modelErrorState.chatItems.length, 1);
assert.equal(modelErrorState.chatItems[0].userError.kind, 'billing');
assert.match(modelErrorState.chatItems[0].text, /DeepSeek账户余额不足/);
assert.doesNotMatch(modelErrorState.chatItems[0].text, /SSE stream request failed/);
assert.match(modelErrorState.chatItems[0].userError.technicalDetail, /\[敏感信息已隐藏\]/);
assert.equal(modelErrorState.chatItems[0].legacyConversationOnly, true);
messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
  { error: rawBillingError },
  modelErrorState,
  addModelErrorItem,
  true,
);
assert.equal(modelErrorState.chatItems.length, 1, 'model service notices must be deduplicated');
assert.equal(
  messageSandbox.window.PinvouBridgeMessages.addModelServiceErrorNotice(
    { error: 'permission denied while reading local file' },
    modelErrorState,
    addModelErrorItem,
    false,
  ),
  false,
  'non-model-service errors must fall back to the raw chat error notice',
);
assert.equal(modelErrorState.chatItems.length, 1, 'non-model-service errors must not add model service notices');

const terminalSandbox = { window: {}, Date };
vm.runInNewContext(modelServiceErrorsSource, terminalSandbox, { filename: 'model-service-errors.js' });
vm.runInNewContext(bridgeMessagesSource, terminalSandbox, { filename: 'bridge-messages.js' });
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
timelineState.activeTurnTimelineId = 'turn-2';
timelineState.turnTimeline.push({ turn_id: 'turn-2', event: 'user_start', ui_turn_index: 3 });
terminalSandbox.window.PinvouWebTurnTerminal.recordCompleted(
  timelineState,
  timelineState.turnTimeline[2],
  { status: 'Failed', error: rawBillingError },
);
assert.equal(timelineState.turnTimeline[3].user_error.kind, 'billing');
assert.match(timelineState.turnTimeline[3].user_error.message, /充值/);

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
assert.match(doneSection, /messages\.addModelServiceErrorNotice/);
assert.match(doneSection, /typeof messages\.addModelServiceErrorNotice === "function"/);
assert.match(doneSection, /shellMessages\.showShellCleanupFailure/);
assert.match(doneSection, /typeof shellMessages\.showShellCleanupFailure === "function"/);
assert.match(
  doneSection,
  /refreshAuthoritativeTurnTimeline\(sid\)/,
  '终态必须重新读取权威时间线，补齐后台或恢复会话漏掉的完成状态',
);
assert.match(
  chatEventsSource,
  /invoke\("get_session_timeline", \{ sessionId: sessionId \}\)/,
  '时间线补偿必须读取当前完成会话，而不是依赖全局 active session',
);
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
assert.match(webBridgeSource, /PinvouBridgeMessages\.addModelServiceErrorNotice/);
assert.equal(
  (bridgeMessagesSource.match(/^    (zh|en|ja):/gm) || []).length,
  3,
  'Shell cleanup warning must provide zh/en/ja translations',
);

// Wave-2 拆分把事件转发(含 Event::TurnComplete 处理)从 engine.rs 移到
// forwarder.rs。契约检查事件处理顺序,故把 forwarder.rs 拼在前面(它含
// TurnComplete→timing→emit 的顺序);未拆分的 main 上没有 forwarder.rs,
// 自动回退为仅 engine.rs。
let engineSource = read('src-tauri', 'src', 'features', 'assistant', 'engine.rs');
try {
  engineSource =
    read('src-tauri', 'src', 'features', 'assistant', 'forwarder.rs') + engineSource;
} catch {
  // main(未拆分)无 forwarder.rs
}
const turnCompleteStart = engineSource.indexOf('Event::TurnComplete');
const turnCompleteSection = engineSource.slice(
  turnCompleteStart,
  engineSource.indexOf('Event::CompactionStarted', turnCompleteStart),
);
assert.ok(
  turnCompleteSection.indexOf('timing::finish_turn_with_usage')
    < turnCompleteSection.indexOf('emit_chat_terminal'),
  '正常完成必须先落权威时间线，再向前端发送 chat:done',
);
const reclaimedSection = engineSource.slice(
  engineSource.indexOf('async fn finish_reclaimed_lifecycle_turn'),
  engineSource.indexOf('impl AppEngine'),
);
assert.ok(
  reclaimedSection.indexOf('timing::finish_turn')
    < reclaimedSection.indexOf('emit_chat_terminal'),
  '回收/中断同样必须先落时间线，再向前端发送终态',
);

vm.runInNewContext(chatEventsSource, sandbox, { filename: 'chat-events.js' });
const installChatEvents = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'];
const authoritativeTimeline = [
  { turn_id: 'disk-old', event: 'user_start', timestamp: 1000 },
  { turn_id: 'disk-old', event: 'assistant_done', timestamp: 4000, status: 'Completed' },
  { turn_id: 'disk-current', event: 'user_start', timestamp: 10000 },
  { turn_id: 'disk-current', event: 'assistant_done', timestamp: 16000, status: 'Completed' },
];
const recoveredTimelineState = {
  turnTimeline: [
    { turn_id: 'ui-current', event: 'user_start', timestamp: 10020, ui_turn_index: 1 },
    { turn_id: 'ui-current', event: 'assistant_done', timestamp: 16020, status: 'Completed', ui_turn_index: 1 },
  ],
};
let timelineNotifyCount = 0;
const chatEvents = installChatEvents({
  state: recoveredTimelineState,
  listen() {},
  invoke(command, args) {
    assert.equal(command, 'get_session_timeline');
    assert.equal(args.sessionId, 'session-recovered');
    return Promise.resolve(authoritativeTimeline);
  },
  runSyncOnSession(sessionId, action) {
    assert.equal(sessionId, 'session-recovered');
    action();
  },
  notify() { timelineNotifyCount += 1; },
  safeConsoleInfo() {},
});
assert.equal(
  await chatEvents.refreshAuthoritativeTurnTimeline('session-recovered'),
  true,
  '后台/恢复会话完成后应接受权威时间线',
);
assert.deepEqual(
  recoveredTimelineState.turnTimeline,
  authoritativeTimeline,
  '权威时间线必须补回本地未见过的早期完成轮次',
);
assert.equal(timelineNotifyCount, 1);

assert.equal(
  chatEvents.authoritativeTimelineMissesKnownCompletion(
    [
      { turn_id: 'turn-current', event: 'user_start', timestamp: 10000 },
      { turn_id: 'turn-current', event: 'assistant_done', timestamp: 11000, status: 'Completed' },
    ],
    [{ turn_id: 'turn-current', event: 'user_start', timestamp: 10000 }],
  ),
  true,
  '短暂滞后的权威快照不得把已完成回合覆盖回执行中',
);

console.log('chat turn error isolation: ok');
