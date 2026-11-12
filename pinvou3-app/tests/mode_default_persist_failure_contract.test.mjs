// 草稿档位默认保存失败传播（评审 JensenChen28 5780647695 + zhuowp 5780927743
// 第二条）的双桥域契约：后端 set_mode_default 持久化失败现在经 IPC 以 Err
// 返回，两座桥（tauri interaction / web bridge）的 catch 必须**保持权威态**
// （不把芯片画成 target、不把旧默认当新值收下）并落显式错误通告；成功保存
// 的对照臂维持现行为（modeDefaults 更新 + 草稿芯片画 target）。后端 Err
// 语义由 Rust 侧 set_mode_default_persist_failure_propagates_and_keeps_mirror
// 与 set_mode_default_command_propagates_persist_failure 钉住。
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const testsRoot = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(testsRoot, '..');
const readApp = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

// ── tauri interaction 特征运行时（chat_bound_workspace_logic 同款 vm harness）──
function loadTauriInteraction(invokeResponder) {
  const storage = new Map();
  const localStorage = {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
    removeItem(key) { storage.delete(key); },
  };
  const root = {};
  const src = readApp('src', 'platform', 'tauri', 'bridge', 'interaction.js');
  vm.runInNewContext(
    readApp('src', 'shared', 'bridge-shared-helpers.js') + '\n' + src,
    { window: root, globalThis: root, localStorage, setTimeout, clearTimeout },
  );
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.interaction;
  const state = {
    activeSessionId: null,
    messages: [], chatItems: [], artifacts: [], queued: [],
    sessions: [], archivedSessions: [],
    scheduledTaskRecentRuns: [], scheduledTaskRuns: [],
    modeState: { mode: 'yolo', multiAgent: false },
    modeDefaults: { work: 'yolo', design: 'yolo', code: null },
    modeLane: 'work',
    draftEpoch: 0,
    composerDraft: '',
    pendingDraftMultiAgent: false,
    pendingDraftMode: null,
    draftWorkspacePath: null,
    busy: false,
  };
  const calls = { invoke: [], notify: 0 };
  const api = factory({
    state,
    sessionStates: {},
    notify() { calls.notify += 1; },
    listen: null,
    bt(key) { return key; },
    addSystemItem(text) { state.chatItems.push({ type: 'system', text, id: 'sys-' + (state.chatItems.length + 1) }); },
    addChatItem(item) { item.id = item.id || ('item-' + (state.chatItems.length + 1)); state.chatItems.push(item); },
    addAuthoritySyncNotice() {},
    timeStr() { return ''; },
    invoke(name, args) {
      calls.invoke.push([name, args]);
      if (invokeResponder) return invokeResponder(name, args);
      if (name === 'set_mode_default') return Promise.resolve(state.modeDefaults);
      return Promise.resolve({ mode: 'yolo', multi_agent: false });
    },
    runSyncOnSession(sid, fn) { fn(); },
    persistMessagesFor() {},
    resetPendingAssistant() {},
    stopThinking() {},
    rerenderFromMessages() {},
    syncModeState() { return Promise.resolve(); },
    currentDraftModeState() {
      const boundDraft = !!state.draftWorkspacePath;
      const lane = boundDraft || state.modeLane === 'code' ? 'code' : 'work';
      const d = state.modeDefaults && state.modeDefaults[lane];
      // 与两座生产桥同构（tauri bridge.js / web bridge.js）：未绑定草稿缺省
      // auto——yolo 兜底会把失败臂的显示面钉在不安全的档上。
      return { mode: d || (boundDraft ? 'plan' : 'auto'), multiAgent: false };
    },
    applyAuthoritativeModeState(sid, st) {
      state.modeState = { mode: st.mode || 'auto', multiAgent: !!st.multi_agent };
    },
    modeStateEpochs: {},
    bumpModeStateEpoch() {},
    syncActivePersona() { return Promise.resolve(); },
    syncMountedCollection() { return Promise.resolve(); },
    reconcileArtifacts() {},
    loadSessionModel() { return Promise.resolve(); },
    turnUsageDirty: {},
    basename(p) { return String(p || '').split('/').pop(); },
    isAbsPath() { return false; },
    filterSessionArtifacts(list) { return list; },
    scheduleShellPoll() {},
    userMessageDisplayText(t) { return t; },
    loadMemoryOverview() { return Promise.resolve(); },
    isScheduledRunSession(id) { return String(id || '').indexOf('sched-') === 0; },
    getBuffer() { return null; },
    flushAssistantMessageToHistory() {},
    ensureSession: async () => (state.activeSessionId || 'chat-a'),
    sendMessage: async () => {},
    reconcileRemoteTurn: async () => true,
    isBusyFor() { return false; },
    markRemoteTurn() {},
  });
  return {
    api, state, calls,
    invokeNames() { return calls.invoke.map(call => call[0]); },
  };
}

// ── web 桥运行时（web_accept_plan_post_accept_gate 同款 vm harness）───────
function bootWebBridge(invokeResponder) {
  const storage = new Map();
  const localStorage = {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
    removeItem(key) { storage.delete(key); },
  };
  const documentObject = {
    readyState: 'loading',
    addEventListener() {},
    createElement() { return { click() {}, remove() {}, style: {}, setAttribute() {} }; },
    body: { appendChild() {} },
  };
  const calls = { invoke: [] };
  const handlers = {
    web_access_list_sessions: () => Promise.resolve([{ id: 'chat-a', title: '新对话', updated_at: 1 }]),
  };
  const listeners = {};
  const windowObject = {
    PinvouPlatform: { kind: 'web', isWeb: true, capabilities: {}, can: () => false, canInvoke: () => false },
    __TAURI__: {
      core: { invoke: (name, args) => {
        calls.invoke.push(name);
        if (invokeResponder) return invokeResponder(name, args);
        if (handlers[name]) return handlers[name](args);
        return Promise.resolve(null);
      } },
      event: { listen: async (name, fn) => { (listeners[name] = listeners[name] || []).push(fn); return function () {}; } },
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
    document: documentObject,
    navigator: { mediaDevices: null },
    localStorage,
    console,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    structuredClone,
    URL,
    URLSearchParams,
    Blob,
    Uint8Array,
    ArrayBuffer,
    TextEncoder,
    TextDecoder,
  });
  vm.runInContext(readApp('src', 'shared', 'bridge-shared-helpers.js'), context, { filename: 'shared/bridge-shared-helpers.js' });
  vm.runInContext(readApp('src', 'platform', 'web', 'bridge.js'), context, { filename: 'platform/web/bridge.js' });
  const flat = windowObject.TauriBridge;
  assert.equal(typeof flat.getState, 'function', 'Web transport must expose its private flat state');
  vm.runInContext(readApp('src', 'platform', 'web', 'bridge', 'domain-adapter.js'), context, { filename: 'platform/web/bridge/domain-adapter.js' });
  return { flat, calls, view() { return flat.getState(); } };
}

const PERSIST_ERROR = new Error('save settings failed: settings.json is a directory');

test('tauri: set_mode_default 持久化失败 → setDraftMode 保持权威态并显式报错', async () => {
  const rt = loadTauriInteraction((name) => {
    if (name === 'set_mode_default') return Promise.reject(PERSIST_ERROR);
    return Promise.resolve({ mode: 'yolo', multi_agent: false });
  });
  const beforeDefaults = JSON.stringify(rt.state.modeDefaults);
  await rt.api.setDraftMode('auto');
  assert.equal(JSON.stringify(rt.state.modeDefaults), beforeDefaults,
    '保存失败不得把（旧/空）默认当成功收下：modeDefaults 不变');
  assert.equal(rt.state.modeState.mode, 'yolo',
    '草稿芯片不得画成 target：失败保持权威态（不切到 auto）');
  assert.equal(rt.state.pendingDraftMode, null, '失败不得暂存显式选择');
  const sys = rt.state.chatItems.filter(item => item.type === 'system');
  assert.ok(sys.some(item => String(item.text).includes('switchModeFailed')),
    '失败必须落显式错误通告');
  assert.ok(rt.invokeNames().includes('set_mode_default'));
});

test('tauri: set_mode_default 成功 → modeDefaults 更新且草稿芯片画 target（对照）', async () => {
  const rt = loadTauriInteraction((name) => {
    if (name === 'set_mode_default') return Promise.resolve({ work: 'auto', code: null });
    return Promise.resolve({ mode: 'yolo', multi_agent: false });
  });
  await rt.api.setDraftMode('auto');
  assert.deepEqual(rt.state.modeDefaults, { work: 'auto', code: null },
    '成功保存以持久化结果更新默认');
  assert.equal(rt.state.modeState.mode, 'auto', '成功切换草稿芯片画 target（现行为不变）');
});

test('web: set_mode_default 持久化失败 → setDraftMode 保持权威态并显式报错', async () => {
  const rt = bootWebBridge((name) => {
    if (name === 'set_mode_default') return Promise.reject(PERSIST_ERROR);
    return Promise.resolve(null);
  });
  // 未绑定 work 草稿的启动基线是解析后的默认档（当前为 auto）；target 取
  // plan，失败与成功才能区分。
  const before = rt.view().modeState.mode;
  await rt.flat.setDraftMode('plan');
  const view = rt.view();
  assert.equal(before, 'auto', 'precondition: boot draft baseline is the resolved default');
  assert.equal(view.modeState.mode, 'auto', '失败保持权威态：草稿芯片不画 target（plan）');
  const sys = view.chatItems.filter(item => item.type === 'system');
  assert.ok(sys.some(item => String(item.text).includes('切') || String(item.text).includes('Failed to switch')),
    '失败必须落显式错误通告（三语码表 switchModeFailed）');
});

test('web: set_mode_default 成功 → 草稿芯片画 target（对照）', async () => {
  const rt = bootWebBridge((name) => {
    if (name === 'set_mode_default') return Promise.resolve({ work: 'plan', code: null });
    return Promise.resolve(null);
  });
  await rt.flat.setDraftMode('plan');
  const view = rt.view();
  assert.deepEqual(view.modeDefaults, { work: 'plan', code: null });
  assert.equal(view.modeState.mode, 'plan', '成功切换草稿芯片画 target（现行为不变）');
});
