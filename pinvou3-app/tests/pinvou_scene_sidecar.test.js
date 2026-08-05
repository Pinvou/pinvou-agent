#!/usr/bin/env node
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const source = fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'chat.js'), 'utf8');
const chatEventsSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'chat-events.js'), 'utf8');
const chatViewSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const tauriBridgeSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge.js'), 'utf8');
const webBridgeSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'web', 'bridge.js'), 'utf8');
const sessionsRustSource = fs.readFileSync(path.join(__dirname, '..', 'src-tauri', 'src', 'app', 'commands', 'sessions.rs'), 'utf8');

// 提取两个 bridge 里真实的 normalizePinvouScene 白名单正则源码，供测试复用，
// 保证 sidecar 记录路径走的是源码里的白名单，而不是各自 mock 的宽松实现。
function extractNormalizeSceneRegex(source) {
  const match = source.match(/return\s*(\/\^.*?\/)\.test\(scene\)/);
  if (!match) throw new Error('normalizePinvouScene 正则未找到');
  return match[1];
}
const tauriNormalizeSceneRegexSource = extractNormalizeSceneRegex(tauriBridgeSource);
const webNormalizeSceneRegexSource = extractNormalizeSceneRegex(webBridgeSource);
// eval 出真正的 RegExp 对象（源码里就是字面量正则）
const tauriNormalizeSceneRegex = eval(tauriNormalizeSceneRegexSource);

function createFeature(options = {}) {
  const sandbox = {
    window: { __PINVOU_TAURI_BRIDGE_FEATURES__: {} },
    console,
  };
  vm.runInNewContext(source, sandbox, { filename: 'bridge/chat.js' });
  const factory = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;
  const state = {
    activeSessionId: 's1',
    messages: [],
    chatItems: [],
    queued: [],
    attachments: [],
    sessions: [{ id: 's1', title: '新对话' }],
    busy: false,
    thinking: { active: false },
  };
  const sessionStates = {
    s1: { queued: state.queued, busy: false, remoteTurnActive: false },
  };
  const invokes = [];
  const sceneEvents = [];
  const context = {
    state,
    invoke(command, args) {
      invokes.push({ command, args });
      if (options.failChat && command === 'chat') {
        return Promise.reject(new Error('admission failed'));
      }
      return Promise.resolve(null);
    },
    notify() {},
    TAURI: { event: { emit() {} } },
    sessionStates,
    turnUsageDirty: {},
    personaPlaceholderTitles: {},
    renderMarkdown(text) { return String(text || ''); },
    safeConsoleInfo() {},
    bt(key) { return key; },
    runSyncOnSession(_sid, fn) { fn(); },
    startThinking() { state.thinking = { active: true }; },
    stopThinking() { state.thinking = { active: false }; },
    ensureSessionBufferLoaded() { return Promise.resolve(); },
    ensureSession() { state.activeSessionId = 's1'; return Promise.resolve('s1'); },
    getBuffer(sid) { return sessionStates[sid]; },
    recordPinvouSceneForMessage(sid, pos, scene) {
      // 走 bridge.js 里真实的白名单门禁：未登记的 scene 会被丢弃为空字符串且不记录，
      // 与 bridge.js:recordPinvouSceneForMessage 的行为保持一致。
      const normalized = tauriNormalizeSceneRegex.test(String(scene || '').trim()) ? scene : '';
      if (!normalized) return;
      const existing = sceneEvents.findIndex(event => event.sid === sid && event.pos === pos);
      if (existing >= 0) sceneEvents.splice(existing, 1);
      sceneEvents.push({ sid, scene: normalized, pos });
    },
    reconcileRemoteTurn() { return Promise.resolve(true); },
    markRemoteTurn() {},
    clearAttachments() {},
    isScheduledRunSession() { return false; },
    basename(value) { return path.basename(String(value || '')); },
    extractArtifactPath() { return ''; },
    parseScheduledTaskDraftFromText() { return null; },
    autoCreateScheduledTaskDraft() { return null; },
    currentStreamText: '',
    currentStreamId: 0,
    pendingAssistantText: '',
    pendingAssistantBlocks: [],
    itemIdSeq: 0,
  };
  return { feature: factory(context), state, sessionStates, invokes, sceneEvents };
}

const results = [];
function rec(name, pass, detail = '') {
  results.push({ name, pass });
  console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`);
}

(async () => {
  {
    const { feature, state, invokes, sceneEvents } = createFeature();
    await feature.doSendFor('s1', '模型 payload', '用户可见文本', [], { pinvouScene: 'work:document-writing' }, false, true);
    const user = state.chatItems.find(item => item.type === 'user');
    const chatInvoke = invokes.find(item => item.command === 'chat');
    rec('发送时用户气泡带 scene，但 messages 不写展示字段',
      user &&
        user.pinvouScene === 'work:document-writing' &&
        state.messages[0] &&
        !Object.prototype.hasOwnProperty.call(state.messages[0], 'pinvouScene') &&
        sceneEvents.length === 1 &&
        sceneEvents[0].pos === 0 &&
        sceneEvents[0].scene === 'work:document-writing' &&
        chatInvoke &&
        chatInvoke.args.message === '模型 payload',
      JSON.stringify({ user, message: state.messages[0], sceneEvents, invokes }));
  }

  {
    const { feature, state, invokes, sceneEvents } = createFeature();
    await feature.sendMessage('运动', {
      pinvouScene: 'work:personal-workbench',
      pinvouPayloadText: '隐藏默认专家 prompt\n\n用户需求：\n运动',
    });
    const user = state.chatItems.find(item => item.type === 'user');
    const chatInvoke = invokes.find(item => item.command === 'chat');
    const messageText = state.messages[0] &&
      state.messages[0].content &&
      state.messages[0].content[0] &&
      state.messages[0].content[0].text;
    rec('个人工作台隐藏 payload 只进模型请求，不进入用户气泡和 messages',
      user &&
        user.text === '运动' &&
        user.pinvouScene === 'work:personal-workbench' &&
        messageText === '运动' &&
        chatInvoke &&
        chatInvoke.args.message === '隐藏默认专家 prompt\n\n用户需求：\n运动' &&
        sceneEvents.length === 1 &&
        sceneEvents[0].scene === 'work:personal-workbench',
      JSON.stringify({ user, message: state.messages[0], chatInvoke, sceneEvents }));
  }

  {
    const { feature, state, sceneEvents } = createFeature({ failChat: true });
    let rejected = false;
    try {
      await feature.doSendFor(
        's1',
        '失败 payload',
        '失败消息',
        [],
        { pinvouScene: 'design:poster' },
        false,
        true,
      );
    } catch (_) {
      rejected = true;
    }
    rec('发送准入失败时不提交 scene sidecar',
      rejected &&
        state.messages.length === 0 &&
        !state.chatItems.some(item => item.type === 'user') &&
        sceneEvents.length === 0,
      JSON.stringify({ rejected, messages: state.messages, chatItems: state.chatItems, sceneEvents }));
  }

  {
    const { feature, state, invokes, sceneEvents } = createFeature();
    state.queued.push(
      { id: 1, text: 'payload 1', displayText: '第一条\n\n📎 ["first.pdf"]', attachments: [{ basename: 'first.pdf' }], meta: { pinvouScene: 'design:poster' }, restrictTools: false },
      { id: 2, text: 'payload 2', displayText: '第二条\n\n📎 ["second.png"]', attachments: [{ basename: 'second.png' }], meta: { pinvouScene: 'design:poster' }, restrictTools: true },
    );
    feature.flushQueued('s1');
    await Promise.resolve();
    const firstUsers = state.chatItems.filter(item => item.type === 'user');
    const firstInvokes = invokes.filter(item => item.command === 'chat');
    const firstPass =
      state.queued.length === 1 && state.queued[0].id === 2 &&
      firstUsers.length === 1 && firstUsers[0].text === '第一条\n\n📎 ["first.pdf"]' &&
      firstUsers[0].pinvouScene === 'design:poster' &&
      firstInvokes.length === 1 && firstInvokes[0].args.message === 'payload 1' &&
      firstInvokes[0].args.attachments.length === 1 &&
      firstInvokes[0].args.attachments[0].basename === 'first.pdf' &&
      firstInvokes[0].args.restrictTools === false;

    state.busy = false;
    feature.flushQueued('s1');
    await Promise.resolve();
    const users = state.chatItems.filter(item => item.type === 'user');
    const chatInvokes = invokes.filter(item => item.command === 'chat');
    rec('queued 多条同一 scene 按 FIFO 分成独立 turn 并各自保留标签',
      firstPass &&
        state.queued.length === 0 &&
        users.length === 2 && users[1].text === '第二条\n\n📎 ["second.png"]' &&
        users[1].pinvouScene === 'design:poster' &&
        chatInvokes.length === 2 && chatInvokes[1].args.message === 'payload 2' &&
        chatInvokes[1].args.attachments.length === 1 &&
        chatInvokes[1].args.attachments[0].basename === 'second.png' &&
        chatInvokes[1].args.restrictTools === true &&
        sceneEvents.length === 2 &&
        sceneEvents.every(event => event.scene === 'design:poster'),
      JSON.stringify({ users, chatInvokes, queued: state.queued, sceneEvents }));
  }

  {
    const { feature, state, invokes, sceneEvents } = createFeature();
    state.queued.push(
      { id: 1, text: 'payload 1', displayText: '第一条', attachments: [], meta: { pinvouScene: 'design:poster' }, restrictTools: false },
      { id: 2, text: 'payload 2', displayText: '第二条', attachments: [], meta: { pinvouScene: 'design:data-visualization' }, restrictTools: false },
    );
    feature.flushQueued('s1');
    await Promise.resolve();
    state.busy = false;
    feature.flushQueued('s1');
    await Promise.resolve();
    const users = state.chatItems.filter(item => item.type === 'user');
    const chatInvokes = invokes.filter(item => item.command === 'chat');
    rec('queued 多条不同 scene 按 FIFO 分发且标签互不污染',
      state.queued.length === 0 &&
        users.length === 2 &&
        users[0].text === '第一条' && users[0].pinvouScene === 'design:poster' &&
        users[1].text === '第二条' && users[1].pinvouScene === 'design:data-visualization' &&
        chatInvokes.length === 2 &&
        chatInvokes[0].args.message === 'payload 1' &&
        chatInvokes[1].args.message === 'payload 2' &&
        sceneEvents.length === 2,
      JSON.stringify({ users, chatInvokes, sceneEvents }));
  }

  {
    const { feature, state } = createFeature({ failChat: true });
    state.queued.push(
      { id: 1, text: 'payload 1', displayText: '第一条', attachments: [], meta: null, restrictTools: false },
      { id: 2, text: 'payload 2', displayText: '第二条', attachments: [], meta: null, restrictTools: false },
    );
    feature.flushQueued('s1');
    await new Promise(resolve => setImmediate(resolve));
    rec('queued 队首发送失败时按原顺序回队且不吞后续消息',
      state.queued.length === 2 &&
        state.queued[0].id === 1 && state.queued[1].id === 2 &&
        !state.chatItems.some(item => item.type === 'user'),
      JSON.stringify({ queued: state.queued, chatItems: state.chatItems }));
  }

  rec('附件-only 发送也会按当前专业子模式创建 scene meta',
    /if \(visibleOutgoing \|\| hasReadyAttachment\)/.test(chatViewSource) &&
      /const scenePrompt = outgoing \|\| '请根据附件内容继续处理。';/.test(chatViewSource) &&
      /\}, \[activeSessionId, dataVisualizationSceneActive, documentWritingSceneActive, hasReadyAttachment, personalWorkbenchSceneActive, t, visualPosterSceneActive\]\);/.test(chatViewSource),
    'ChatView sendChatMessage contract');

  rec('scene sidecar 通过 session 后端在 Tauri/Web 间共享并保留本地迁移缓存',
    /get_session_pinvou_scene_events/.test(tauriBridgeSource) &&
      /save_session_pinvou_scene_events/.test(tauriBridgeSource) &&
      /syncPinvouSceneEventsForSession/.test(tauriBridgeSource) &&
      /get_session_pinvou_scene_events/.test(webBridgeSource) &&
      /save_session_pinvou_scene_events/.test(webBridgeSource) &&
      /syncPinvouSceneEventsForSession/.test(webBridgeSource) &&
      /pub async fn save_session_pinvou_scene_events/.test(sessionsRustSource) &&
      /pub async fn get_session_pinvou_scene_events/.test(sessionsRustSource),
    'shared session scene sidecar contract');

  rec('三个场景白名单(Tauri/Web/Rust)必须同时登记 work:personal-workbench，否则 sidecar 重载后会丢标签',
    tauriNormalizeSceneRegexSource.includes('work:personal-workbench') &&
      webNormalizeSceneRegexSource.includes('work:personal-workbench') &&
      /Some\("work:personal-workbench"\) => "work:personal-workbench"/.test(sessionsRustSource),
    'normalize allowlist must register work:personal-workbench across tauri bridge, web bridge and Rust backend');

  rec('远程消息不会越过已有 FIFO 队列',
    /var remoteBuffer = getBuffer\(sid\);/.test(chatEventsSource) &&
      /isBusyFor\(sid\) \|\| \(remoteBuffer && remoteBuffer\.queued && remoteBuffer\.queued\.length > 0\)/.test(chatEventsSource) &&
      /if \(!isBusyFor\(sid\)\) flushQueued\(sid\);/.test(chatEventsSource),
    'mobile user messages must enqueue behind pending local turns');

  const failed = results.filter(item => !item.pass);
  if (failed.length) {
    console.error(`\n❌ FAIL ${failed.length}/${results.length}`);
    process.exit(1);
  }
  console.log(`\n✅ ALL ${results.length} PASS`);
})();
