// Issue #406 回归：sendMessage 的「仅提示、不派发」早退路径曾 resolve undefined，
// ChatView.handleSend 把它当 accepted=true，用户输入框草稿被静默丢弃。
// 修复后 sendMessage 必须按返回协议报告每次退出：
//   true        已派发（发出 / steer chip / 入队）
//   "restored"  未派发，但文本已被 bridge 放回输入框（调用方不得再恢复，否则重复）
//   false       未派发且未恢复（调用方走 handleSend 既有的 empty-vs-typed 恢复）
// 覆盖三面：tauri bridge 行为、web bridge 行为、ChatView 映射与恢复逻辑的源码契约。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

async function flushMicrotasks(rounds = 8) {
  for (let i = 0; i < rounds; i += 1) await Promise.resolve();
}

// 有界轮询：ensureSession 物化链有多个 await，固定轮数对不上微任务节拍。
async function waitFor(predicate, label) {
  for (let i = 0; i < 200; i += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  assert.ok(predicate(), `waitFor 超时：${label}`);
}

// 挂起用例的"发射后不管"：promise 永不结算，也要兜住万一的 rejection，
// 不让未处理 rejection 打断测试进程。
function forget(promise) {
  promise.catch(() => {});
}

// ── tauri bridge：vm 加载 chat.js 特性工厂 + mock context ─────────────
function createTauriChat(overrides = {}) {
  const sandbox = {
    window: { __PINVOU_TAURI_BRIDGE_FEATURES__: {} },
    console,
    setTimeout,
    clearTimeout,
  };
  vm.runInNewContext(read('src', 'platform', 'tauri', 'bridge', 'chat.js'), sandbox, {
    filename: 'bridge/chat.js',
  });
  const factory = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;
  const state = {
    activeSessionId: 's1',
    messages: [],
    chatItems: [],
    queued: [],
    attachments: [],
    sessions: [{ id: 's1' }, { id: 's2' }],
    busy: false,
    composerDraft: '',
    composerPrefill: { id: 0, text: '', append: false },
    draftEpoch: 0,
  };
  const sessionStates = {
    s1: { queued: state.queued, busy: false, remoteTurnActive: false, composerDraft: '' },
    s2: { queued: [], busy: false, remoteTurnActive: false, composerDraft: '' },
  };
  const discarded = [];
  const context = {
    state,
    invoke: overrides.invoke || (async () => null),
    notify() {},
    TAURI: { event: { emit() {} } },
    sessionStates,
    turnUsageDirty: {},
    personaPlaceholderTitles: {},
    renderMarkdown: text => String(text || ''),
    safeConsoleInfo() {},
    bt(key) { return key; },
    isDefaultChatTitle() { return false; },
    runSyncOnSession(_sid, fn) { fn(); },
    startThinking() {},
    stopThinking() {},
    ensureSessionBufferLoaded() { return Promise.resolve(); },
    ensureSession: overrides.ensureSession || (() => Promise.resolve('s1')),
    getBuffer(sid) { return sessionStates[sid]; },
    recordPinvouSceneForMessage() {},
    recordSteeredMessages() {},
    reconcileRemoteTurn: overrides.reconcileRemoteTurn || (() => Promise.resolve(true)),
    markRemoteTurn() {},
    isScheduledRunSession() { return false; },
    adoptManagedAttachments: overrides.adoptManagedAttachments || (() => Promise.resolve()),
    discardManagedAttachment(result) { discarded.push(result); },
    userMessageDisplayText(blocks) {
      return (Array.isArray(blocks) ? blocks : [])
        .filter(block => block && block.type === 'text')
        .map(block => String(block.text || ''))
        .join('');
    },
    parseScheduledTaskDraftFromText() { return null; },
    autoCreateScheduledTaskDraft() { return null; },
    currentStreamText: '',
    currentStreamId: 0,
    pendingAssistantText: '',
    pendingAssistantBlocks: [],
    itemIdSeq: 0,
  };
  return { feature: factory(context), state, sessionStates, discarded };
}

const hasNotice = (state, text) => state.chatItems.some(
  item => item && item.type === 'system' && item.text === text,
);

// ── web bridge：vm 加载 bridge.js + domain-adapter.js，公共 API 驱动 ──
function createWebBridge() {
  const storage = new Map();
  const localStorage = {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
    removeItem(key) { storage.delete(key); },
  };
  const pendingInvokes = [];
  let invokeResponse = async () => null;
  let canInvokeResult = false;
  const windowObject = {
    PinvouPlatform: {
      kind: 'web',
      isWeb: true,
      capabilities: {},
      can: () => false,
      canInvoke: () => canInvokeResult,
    },
    __TAURI__: {
      core: { invoke: (...args) => invokeResponse(...args) },
      event: { listen: async () => () => {} },
      dialog: { open: async () => null },
    },
    location: { search: '', href: 'https://example.test/pinvou3/remote/' },
    localStorage,
    crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
    performance: { now: () => 0 },
    addEventListener() {},
    setTimeout,
    clearTimeout,
  };
  const context = vm.createContext({
    window: windowObject,
    document: {
      readyState: 'loading',
      addEventListener() {},
      createElement() { return { click() {}, remove() {}, style: {}, setAttribute() {} }; },
      body: { appendChild() {} },
    },
    navigator: { mediaDevices: null },
    localStorage,
    console,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    structuredClone: value => structuredClone(value),
    URL,
    URLSearchParams,
    Blob,
    Uint8Array,
    ArrayBuffer,
    TextEncoder,
    TextDecoder,
  });
  vm.runInContext(read('src', 'platform', 'web', 'bridge.js'), context, {
    filename: 'platform/web/bridge.js',
  });
  vm.runInContext(read('src', 'platform', 'web', 'bridge', 'domain-adapter.js'), context, {
    filename: 'platform/web/bridge/domain-adapter.js',
  });
  const api = windowObject.TauriBridge;
  return {
    api,
    setInvokeResponse(fn) { invokeResponse = fn; },
    setCanInvoke(value) { canInvokeResult = value; },
    chatState: () => api.state.get('chat'),
    // 制造一个停在解析中的附件（底层 invoke 永不结算，状态停在 parsing）。
    addParsingAttachment() {
      invokeResponse = () => new Promise(() => {});
      return api.attachments.addAttachmentByPath('/tmp/parse-forever.txt');
    },
    async addReadyAttachment() {
      invokeResponse = async command => (command === 'web_access_ingest_file'
        ? { basename: 'ready.txt', handle: 'handle-ready' }
        : null);
      await api.attachments.addAttachmentByPath('/tmp/ready.txt');
    },
    pendingInvokes,
  };
}

const chatViewSource = read('src', 'features', 'chat', 'ChatView.jsx');

// ── tauri 行为 ────────────────────────────────────────────────────────
{
  // 附件仍在解析：仅系统提示，不派发 → false（调用方恢复草稿）。
  const { feature, state } = createTauriChat();
  state.attachments.push({ status: 'parsing' });
  const result = await feature.sendMessage('你好', null);
  assert.equal(result, false, 'tauri parsing 附件路径必须返回 false');
  assert.ok(hasNotice(state, 'attachStillParsing'), '必须保留 attachStillParsing 系统提示');
  assert.equal(state.messages.length, 0, '不得派发任何消息');
  assert.equal(state.queued.length, 0, '不得入队');
}

{
  // 远端回合对账失败（同会话）：仅权威同步提示 → false。
  const { feature, state, sessionStates } = createTauriChat({
    reconcileRemoteTurn: () => Promise.resolve(false),
  });
  sessionStates.s1.remoteTurnActive = true;
  const result = await feature.sendMessage('你好', null);
  assert.equal(result, false, 'tauri 远端对账失败（同会话）必须返回 false');
  assert.ok(
    state.chatItems.some(item => item && item.authoritySyncNotice && item.text === 'remoteTurnSyncing'),
    '必须保留 remoteTurnSyncing 提示',
  );
  assert.equal(state.messages.length, 0, '不得派发任何消息');
}

{
  // 远端对账失败 + await 期间切走：文本回到原会话 buffer 草稿 → "restored"，
  // 附件被放弃，且不得泄入当前会话（s2）的工作集。
  const { feature, state, sessionStates, discarded } = createTauriChat({
    reconcileRemoteTurn: () => {
      state.activeSessionId = 's2'; // await 期间用户切到 s2
      return Promise.resolve(false);
    },
  });
  sessionStates.s1.remoteTurnActive = true;
  state.attachments.push({ status: 'ready', result: { handle: 'h1' } });
  const result = await feature.sendMessage('你好', null);
  assert.equal(result, 'restored', 'tauri 切走路径必须返回 "restored"');
  assert.equal(sessionStates.s1.composerDraft, '你好', '文本必须回到原会话 s1 的草稿');
  assert.equal(state.composerDraft, '', '不得把文本泄入当前会话 s2 的工作集');
  assert.equal(discarded.length, 1, '准备好的附件必须被放弃');
  assert.equal(state.messages.length, 0, '不得派发任何消息');
}

{
  // 草稿态物化中止：bridge 已 prefill 恢复 → "restored"（调用方再恢复会重复）。
  const { feature, state } = createTauriChat({
    ensureSession: () => {
      state.activeSessionId = null;
      return Promise.resolve(null);
    },
  });
  state.activeSessionId = null;
  const result = await feature.sendMessage('你好', null);
  assert.equal(result, 'restored', 'tauri 物化中止路径必须返回 "restored"');
  assert.equal(state.composerPrefill.text, '你好', 'prefill 必须携带原文');
  assert.equal(state.composerPrefill.append, true, 'prefill 必须是 append 语义');
}

{
  // 附件托管失败 + await 期间切走：→ "restored"，文本回原会话。
  const { feature, state, sessionStates, discarded } = createTauriChat({
    adoptManagedAttachments: () => {
      state.activeSessionId = 's2';
      return Promise.reject(new Error('adopt failed'));
    },
  });
  state.attachments.push({ status: 'ready', result: { handle: 'h1' } });
  const result = await feature.sendMessage('你好', null);
  assert.equal(result, 'restored', 'tauri adopt 失败 + 切走必须返回 "restored"');
  assert.equal(sessionStates.s1.composerDraft, '你好', '文本必须回到原会话 s1 的草稿');
  assert.equal(discarded.length, 1, '附件必须被放弃');
}

{
  // 附件托管失败（同会话）：仅系统提示 → false。
  const { feature, state } = createTauriChat({
    adoptManagedAttachments: () => Promise.reject(new Error('boom')),
  });
  const result = await feature.sendMessage('你好', null);
  assert.equal(result, false, 'tauri adopt 失败（同会话）必须返回 false');
  assert.ok(
    state.chatItems.some(item => item && item.type === 'system' && String(item.text).startsWith('deviceUploadFailed')),
    '必须保留 deviceUploadFailed 提示',
  );
  assert.equal(state.messages.length, 0, '不得派发任何消息');
}

{
  // 生成中发送：steer chip 派发 → true。
  const { feature, state } = createTauriChat();
  state.busy = true; // isBusyFor('s1')：sid 即 active，读 state.busy
  const result = await feature.sendMessage('插一句', null);
  assert.equal(result, true, 'tauri busy steer 派发必须返回 true');
  assert.equal(state.queued.length, 1, '必须产生 steer chip');
  assert.equal(state.queued[0].steered, true, 'chip 必须标记 steered');
}

{
  // 常规发送：doSendFor 受理 → true。
  const { feature, state } = createTauriChat();
  const result = await feature.sendMessage('你好', null);
  assert.equal(result, true, 'tauri 常规派发必须返回 true');
  assert.equal(state.messages.length, 1, 'user 消息必须进入 transcript');
}

{
  // 主路径失败仍必须 reject（surfaceFailure 契约不变，调用方走 catch 恢复）。
  const { feature } = createTauriChat({
    invoke: async (command) => {
      if (command === 'chat') throw new Error('reserve conflict');
      return null;
    },
  });
  await assert.rejects(
    () => feature.sendMessage('你好', null),
    /reserve conflict/,
    'tauri 主路径失败必须保持抛出',
  );
}

{
  // 空文本 + 无附件：无可派发 → false（无草稿可恢复，恢复分支自然空操作）。
  const { feature } = createTauriChat();
  const result = await feature.sendMessage('', null);
  assert.equal(result, false, 'tauri 空发送必须返回 false');
}

// ── web 行为 ──────────────────────────────────────────────────────────
{
  // 附件仍在解析 → false（web bt 是内置真实三语文案，断言用条目形状而非文本）。
  // addParsingAttachment 的底层 invoke 永不结算：不 await（无 rejection 路径），
  // 只等微任务让状态同步到 parsing。
  const web = createWebBridge();
  web.addParsingAttachment();
  await flushMicrotasks(3);
  const chatBefore = web.chatState();
  assert.equal(chatBefore.attachments[0].status, 'parsing', 'web 前置：附件处于 parsing');
  const systemNoticesBefore = chatBefore.chatItems.filter(item => item && item.type === 'system').length;
  const result = await web.api.chat.sendMessage('你好', null);
  assert.equal(result, false, 'web parsing 附件路径必须返回 false');
  const chatAfter = web.chatState();
  const systemNoticesAfter = chatAfter.chatItems.filter(item => item && item.type === 'system').length;
  assert.equal(systemNoticesAfter, systemNoticesBefore + 1, 'web 必须新增一条解析中系统提示');
  assert.equal(chatAfter.messages.length, 0, 'web 不得派发任何消息');
}

{
  // 草稿态首条（web_access 通道）已存在 → 竞态静默路径必须 false。
  // first 永不结算（invoke 挂起），不能 await（顶层 await 会卡住测试进程）。
  const web = createWebBridge();
  web.setCanInvoke(true);
  web.setInvokeResponse(() => new Promise(() => {})); // 首条挂起 → chatItems 留下 deliveryState 用户气泡
  const first = web.api.chat.sendMessage('第一条', null);
  forget(first);
  await flushMicrotasks(3);
  const second = await web.api.chat.sendMessage('第二条', null);
  assert.equal(second, false, 'web existingFirstTurn 竞态必须返回 false');
}

{
  // 草稿态物化中止（create_session 失败）：bridge 已 prefill → "restored"。
  const web = createWebBridge();
  web.setInvokeResponse(async (command) => {
    if (command === 'web_access_create_session') throw new Error('create failed');
    return null;
  });
  const result = await web.api.chat.sendMessage('你好', null);
  assert.equal(result, 'restored', 'web 物化中止路径必须返回 "restored"');
  const chat = web.chatState();
  assert.equal(chat.composerPrefill.text, '你好', 'web prefill 必须携带原文');
  assert.equal(chat.composerPrefill.append, true, 'web prefill 必须是 append 语义');
}

{
  // 生成中发送：入队 chip → true。首条发送经 ensureSession 物化会话后挂在
  // web_access_chat 上撑住 busy；first 永不结算，不能 await。
  const web = createWebBridge();
  web.setInvokeResponse(async (command) => {
    if (command === 'web_access_create_session') return { id: 'web-1' };
    if (command === 'web_access_chat') return new Promise(() => {});
    return null;
  });
  const first = web.api.chat.sendMessage('第一条', null);
  forget(first);
  await waitFor(() => web.chatState().busy, '首条发送应进入 busy');
  const second = await web.api.chat.sendMessage('第二条', null);
  assert.equal(second, true, 'web busy 入队派发必须返回 true');
  assert.equal(web.chatState().queued.length, 1, 'web 必须产生待发 chip');
}

{
  // 受理被拒（web_access_chat 拒绝）：仅错误提示 → false（调用方恢复草稿）。
  // 首条发送经 ensureSession 内联物化会话（createNewSession 的 mock 响应不足以
  // 完成 switchActiveTo，这里不依赖它）。
  const web = createWebBridge();
  web.setInvokeResponse(async (command) => {
    if (command === 'web_access_create_session') return { id: 'web-1' };
    if (command === 'web_access_chat') throw new Error('admission rejected');
    return null;
  });
  const result = await web.api.chat.sendMessage('你好', null);
  assert.equal(result, false, 'web 受理被拒必须返回 false');
  const chat = web.chatState();
  assert.ok(
    chat.chatItems.some(item => item && item.turnErrorNotice),
    'web 必须保留受理失败提示',
  );
  assert.equal(chat.messages.length, 0, 'web 失败不得残留乐观消息');
}

{
  // 常规发送 → true（同上，经 ensureSession 内联物化）。
  const web = createWebBridge();
  web.setInvokeResponse(async (command) => {
    if (command === 'web_access_create_session') return { id: 'web-1' };
    if (command === 'web_access_chat') return null;
    return null;
  });
  const result = await web.api.chat.sendMessage('你好', null);
  assert.equal(result, true, 'web 常规派发必须返回 true');
  assert.equal(web.chatState().messages.length, 1, 'web user 消息必须进入 transcript');
}

// ── ChatView 源码契约 ─────────────────────────────────────────────────
// sendChatMessage 必须把 sendMessage 的返回协议映射给 handleSend：
// 仅 false 触发恢复；"restored"/true/undefined（旧后端兜底）都不得触发。
assert.match(
  chatViewSource,
  /dispatchResult = await bridge\.chat\.sendMessage\(visibleOutgoing, meta\);[\s\S]*?return dispatchResult !== false;/,
  'sendChatMessage 必须按 dispatchResult !== false 映射 sendMessage 的返回协议',
);
// handleSend 的恢复必须保留 empty-vs-typed 区分（空输入框整体还原，非空降级为 append prefill）。
assert.match(
  chatViewSource,
  /if \(!accepted\) \{\s*\n\s*if \(inputTextRef\.current === ''\) setInputText\(text\);\s*\n\s*else if \(text\) bridge\.chat\.prefillComposer\(text, true\);/,
  'handleSend 的 !accepted 恢复分支必须保留 empty-vs-typed 区分',
);

console.log('send_draft_restore_logic: all assertions passed');
