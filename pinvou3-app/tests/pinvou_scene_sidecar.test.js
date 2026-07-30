#!/usr/bin/env node
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const source = fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'chat.js'), 'utf8');
const chatViewSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const tauriBridgeSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge.js'), 'utf8');
const webBridgeSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'web', 'bridge.js'), 'utf8');
const sessionsRustSource = fs.readFileSync(path.join(__dirname, '..', 'src-tauri', 'src', 'app', 'commands', 'sessions.rs'), 'utf8');

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
      const existing = sceneEvents.findIndex(event => event.sid === sid && event.pos === pos);
      if (existing >= 0) sceneEvents.splice(existing, 1);
      sceneEvents.push({ sid, scene, pos });
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
    const { feature, state, sceneEvents } = createFeature();
    state.queued.push(
      { id: 1, text: 'payload 1', displayText: '第一条', attachments: [], meta: { pinvouScene: 'design:poster' }, restrictTools: false },
      { id: 2, text: 'payload 2', displayText: '第二条', attachments: [], meta: { pinvouScene: 'design:poster' }, restrictTools: false },
    );
    feature.flushQueued('s1');
    await Promise.resolve();
    const user = state.chatItems.find(item => item.type === 'user');
    rec('queued 多条同一 scene 合并后保留只读标签',
      user &&
        user.text === 'payload 1\n\npayload 2' &&
        user.pinvouScene === 'design:poster' &&
        sceneEvents.length === 1 &&
        sceneEvents[0].scene === 'design:poster',
      JSON.stringify({ user, sceneEvents }));
  }

  {
    const { feature, state, sceneEvents } = createFeature();
    state.queued.push(
      { id: 1, text: 'payload 1', displayText: '第一条', attachments: [], meta: { pinvouScene: 'design:poster' }, restrictTools: false },
      { id: 2, text: 'payload 2', displayText: '第二条', attachments: [], meta: { pinvouScene: 'design:data-visualization' }, restrictTools: false },
    );
    feature.flushQueued('s1');
    await Promise.resolve();
    const user = state.chatItems.find(item => item.type === 'user');
    rec('queued 多条不同 scene 合并后不误标标签',
      user &&
        user.text === 'payload 1\n\npayload 2' &&
        !user.pinvouScene &&
        sceneEvents.length === 0,
      JSON.stringify({ user, sceneEvents }));
  }

  rec('附件-only 发送也会按当前专业子模式创建 scene meta',
    /if \(outgoing \|\| hasReadyAttachment\)/.test(chatViewSource) &&
      /const scenePrompt = outgoing \|\| '请根据附件内容继续处理。';/.test(chatViewSource) &&
      /\}, \[activeSessionId, dataVisualizationSceneActive, documentWritingSceneActive, hasReadyAttachment, visualPosterSceneActive\]\);/.test(chatViewSource),
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

  const failed = results.filter(item => !item.pass);
  if (failed.length) {
    console.error(`\n❌ FAIL ${failed.length}/${results.length}`);
    process.exit(1);
  }
  console.log(`\n✅ ALL ${results.length} PASS`);
})();
