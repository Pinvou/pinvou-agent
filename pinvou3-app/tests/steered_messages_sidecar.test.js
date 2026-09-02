#!/usr/bin/env node
// Steer 落盘对齐的展示层配套：steer 消息持久化为普通展示副本（无 <turn_meta>
// 尾块），重载投影靠 steered-messages sidecar 的 {pos, text} 恢复
// steeredMidTurn 标记。本测试覆盖 chat.js 侧的结算记录路径：
//   settleSteerCommitted → 待定位 → load_session 快照尾对齐匹配 →
//   recordSteeredMessages({pos, text})，以及未落盘重试与 purge 清理。
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const chatSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'chat.js'), 'utf8');

function createFeature(options = {}) {
  const sandbox = {
    window: { __PINVOU_TAURI_BRIDGE_FEATURES__: {} },
    console,
    setTimeout,
    clearTimeout,
  };
  vm.runInNewContext(chatSource, sandbox, { filename: 'bridge/chat.js' });
  const factory = sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__.chat;
  const state = {
    activeSessionId: 's1',
    messages: [],
    chatItems: [],
    queued: [],
    attachments: [],
    sessions: [{ id: 's1', title: '新对话' }],
    busy: true,
    thinking: { active: true },
  };
  const sessionStates = {
    s1: { queued: state.queued, busy: true, remoteTurnActive: false },
  };
  const invokes = [];
  const recorded = [];
  // 模拟 Rust 侧已落盘的权威 transcript；测试用例按场景替换。
  let savedMessages = options.savedMessages || [];
  const context = {
    state,
    invoke(command, args) {
      invokes.push({ command, args });
      if (command === 'load_session') {
        return Promise.resolve({ metadata: { id: args.id }, messages: savedMessages });
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
    isDefaultChatTitle() { return false; },
    runSyncOnSession(_sid, fn) { fn(); },
    startThinking() {},
    stopThinking() {},
    ensureSessionBufferLoaded() { return Promise.resolve(); },
    ensureSession() { return Promise.resolve('s1'); },
    getBuffer(sid) { return sessionStates[sid]; },
    recordPinvouSceneForMessage() {},
    recordSteeredMessages(sid, entries) { recorded.push({ sid, entries }); },
    reconcileRemoteTurn() { return Promise.resolve(true); },
    markRemoteTurn() {},
    isScheduledRunSession() { return false; },
    userMessageDisplayText(blocks) {
      return (Array.isArray(blocks) ? blocks : [])
        .filter(block => block && block.type === 'text')
        .map(block => String(block.text || ''))
        .filter(text => {
          const t = text.trim();
          return !(t.indexOf('<turn_meta>') === 0 && t.endsWith('</turn_meta>'));
        })
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
  return {
    feature: factory(context),
    state,
    invokes,
    recorded,
    setSavedMessages(messages) { savedMessages = messages; },
  };
}

function userMessage(text) {
  return { role: 'user', content: [{ type: 'text', text }] };
}

function assistantMessage(text) {
  return { role: 'assistant', content: [{ type: 'text', text }] };
}

const results = [];
function rec(name, pass, detail = '') {
  results.push({ name, pass });
  console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`);
}

async function flushMicrotasks() {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
}

// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke script keeps its existing async main() structure
(async () => {
  {
    // 结算时快照已含 steer（展示副本形态）：尾对齐匹配记录其落盘位置。
    const { feature, state, recorded } = createFeature({
      savedMessages: [
        userMessage('先把按钮做出来'),
        assistantMessage('好的'),
        userMessage('改成红色'),
      ],
    });
    state.queued.push({ id: 1, text: '改成红色', steered: true, steerId: 'st-1' });
    feature.settleSteerCommitted('s1', 'st-1');
    await flushMicrotasks();
    const bubble = state.chatItems.find(item => item.type === 'user');
    rec('steer 结算后立即打泡并按快照记录 sidecar 位置',
      !!bubble &&
        bubble.steeredMidTurn === true &&
        state.queued.length === 0 &&
        recorded.length === 1 &&
        recorded[0].sid === 's1' &&
        recorded[0].entries.length === 1 &&
        recorded[0].entries[0].pos === 2 &&
        recorded[0].entries[0].text === '改成红色',
      JSON.stringify({ bubble, queued: state.queued, recorded }));
  }

  {
    // 历史里存在同文 admission：尾对齐必须命中最后一条（steer），不得误配旧消息。
    const { feature, state, recorded } = createFeature({
      savedMessages: [
        userMessage('继续'),
        assistantMessage('第一轮回答'),
        userMessage('继续'),
      ],
    });
    state.queued.push({ id: 1, text: '继续', steered: true, steerId: 'st-2' });
    feature.settleSteerCommitted('s1', 'st-2');
    await flushMicrotasks();
    rec('同文历史存在时尾对齐记录 steer 而非旧 admission',
      recorded.length === 1 && recorded[0].entries[0].pos === 2,
      JSON.stringify(recorded));
  }

  {
    // 结算先于落盘（steer_committed 早于 transcript 持久化）：首次快照没有
    // steer，不记录；transcript_committed 重试（captureSteerPositions）后记录。
    const { feature, state, recorded, setSavedMessages } = createFeature({
      savedMessages: [userMessage('先把按钮做出来')],
    });
    state.queued.push({ id: 1, text: '改成红色', steered: true, steerId: 'st-3' });
    feature.settleSteerCommitted('s1', 'st-3');
    await flushMicrotasks();
    const notYet = recorded.length === 0;
    setSavedMessages([userMessage('先把按钮做出来'), userMessage('改成红色')]);
    feature.captureSteerPositions('s1');
    await flushMicrotasks();
    rec('落盘晚于结算时挂起，transcript_committed 重试后补齐位置',
      notYet &&
        recorded.length === 1 &&
        recorded[0].entries[0].pos === 1 &&
        recorded[0].entries[0].text === '改成红色',
      JSON.stringify({ notYet, recorded }));
  }

  {
    // purgeSteerState 清空待定位队列：会话删除后迟到的快照不得再写 sidecar。
    const { feature, state, recorded, invokes } = createFeature({
      savedMessages: [userMessage('先把按钮做出来')],
    });
    state.queued.push({ id: 1, text: '改成红色', steered: true, steerId: 'st-4' });
    feature.settleSteerCommitted('s1', 'st-4');
    await flushMicrotasks();
    feature.purgeSteerState('s1');
    invokes.length = 0;
    feature.captureSteerPositions('s1');
    await flushMicrotasks();
    rec('purge 后待定位 steer 不再触发快照读取与记录',
      recorded.length === 0 && invokes.every(item => item.command !== 'load_session'),
      JSON.stringify({ recorded, invokes }));
  }

  const failed = results.filter(result => !result.pass);
  console.log(`\n${results.length - failed.length}/${results.length} passed`);
  if (failed.length) process.exit(1);
})();
