/**
 * 会话 buffer LRU 淘汰回归测试（PR #339 五审 P1/P2）：
 *
 * P1 —— 全会话 LRU 淘汰空闲 buffer 时连同 composerDraft 一起丢弃。磁盘
 * transcript 只含已提交内容，草稿（Tauri 与 Web 均不落盘）一旦淘汰即从
 * 「可恢复」变「永久丢失」。修复：淘汰前把不可重水化的轻量草稿转移到
 * evictedSessionDrafts 侧表（独立上限 256），buffer 重建时回填；真实会话
 * 删除（purgeSessionBuffer）时作废，不得回流。
 *
 * P2 —— LRU 淘汰把 scene 事件的 localStorage 缓存键当会话删除清理。该缓存
 * 是 sidecar 保存失败/离线时的唯一恢复副本（savePinvouSceneEventsForSession
 * 的后端失败被有意吞掉，syncPinvouSceneEventsForSession 靠它兜底重放）。
 * 修复：容量淘汰（reason="evict"）保留缓存键，仅真实会话删除
 * （reason="delete"）清理。
 *
 * tauri 侧经 sessions.js factory 注入（同 session_nav_race.test.mjs），
 * web 侧整桥 vm 装载（同 session_nav_race_web.test.mjs）驱动公开 API
 * switchToSession/setComposerDraft/deleteSession 走生产路径。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');
const webBridgeRoot = path.resolve(here, '..', 'src', 'platform', 'web');
const read = relativePath => fs.readFileSync(path.join(webBridgeRoot, relativePath), 'utf8');

// ── tauri sessions.js factory 装载 ────────────────────────────────

function loadTauriSessionsFeature(overrides) {
  const root = { __PINVOU_SHARED_I18N__: {} };
  const src = fs.readFileSync(path.join(bridgeDir, 'sessions.js'), 'utf8');
  vm.runInNewContext(src, { window: root, globalThis: root, setTimeout, clearTimeout });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.sessions;
  const state = {
    activeSessionId: null,
    messages: [], chatItems: [], artifacts: [], queued: [],
    sessions: [], archivedSessions: [],
    scheduledTaskRecentRuns: [], scheduledTaskRuns: [],
    modeState: { mode: 'yolo', multiAgent: false },
    modeDefaults: { work: 'yolo', design: 'yolo' },
    modeLane: 'work',
    draftEpoch: 0,
    composerDraft: '',
    pendingDraftMultiAgent: false,
    scheduledRunContext: null,
    scheduledTaskPendingGuide: null,
    mountedCollections: [],
    mountedCollectionsRevision: 0,
    busy: false,
    thinking: false,
    tokens: { input: 0, max: 0 },
    turnTimeline: [],
    activeTurnTimelineId: null,
    personaEvents: [],
    pinvouReviews: [],
    pinvouSceneEvents: [],
    scheduledTaskDraft: null,
  };
  const sessionStates = {};
  const purgeLog = [];
  const calls = { invoke: [] };
  const api = factory(Object.assign({
    state,
    sessionStates,
    notify() { calls.notify = (calls.notify || 0) + 1; },
    listen: null,
    bt(key) { return key; },
    onSessionBufferPurged(id, reason) { purgeLog.push({ id, reason }); },
    addSystemItem(text) { state.chatItems.push({ type: 'system', text }); },
    addChatItem(item) { state.chatItems.push(item); },
    timeStr() { return ''; },
    invoke(name, args) {
      calls.invoke.push(name);
      return Promise.resolve({});
    },
    runSyncOnSession(sid, fn) { fn(); },
    persistMessagesFor() {},
    resetPendingAssistant() {},
    stopThinking() {},
    rerenderFromMessages() {},
    syncModeState() { return Promise.resolve(); },
    applyAuthoritativeModeState() {},
    currentDraftModeState() { return { mode: 'yolo', multiAgent: false }; },
    syncActivePersona() { return Promise.resolve(); },
    syncMountedCollection() { return Promise.resolve(); },
    reconcileArtifacts() {},
    loadSessionModel() { return Promise.resolve(); },
    clearScheduledTaskSelection() {},
    invalidateScheduledRecentRunsForSession() {},
    refreshHistoryListForRun() { return Promise.resolve(); },
    turnUsageDirty: {},
    basename(p) { return String(p || '').split('/').pop(); },
    isAbsPath() { return false; },
    filterSessionArtifacts(list) { return list; },
    scheduleShellPoll() {},
    setScheduledTaskError() {},
    userMessageDisplayText(t) { return t; },
    loadMemoryOverview() { return Promise.resolve(); },
    isScheduledRunSession(id) { return String(id || '').indexOf('sched-') === 0; },
    invalidateScheduledTaskReads() {},
    applyScheduledRunViewed() {},
    loadScheduledTaskRecentRuns() { return Promise.resolve(); },
    scheduledRunSessionOwners: {},
    personaPlaceholderTitles: {},
    currentStreamText: '',
    currentStreamId: 0,
    pendingAssistantText: '',
    pendingAssistantBlocks: [],
    itemIdSeq: 0,
    toolMeta: {},
  }, overrides || {}));
  return { api, state, sessionStates, purgeLog, calls };
}

// ── P1：33 会话淘汰后草稿回填（tauri）──────────────────────────────

test('tauri 33 会话 LRU：淘汰最旧空闲 buffer，草稿经侧表回填，buffer 数有界', () => {
  const { api, state, sessionStates, purgeLog } = loadTauriSessionsFeature();
  const total = 33;
  for (let i = 1; i <= total; i++) {
    api.switchActiveTo(`s${i}`, null);
    // 切入后输入：离开 s_i 时 saveWorkingSetTo 快照的才是 draft-i（生产语义）。
    state.composerDraft = `draft-${i}`;
    state.messages = [{ role: 'user', content: `m${i}` }];
  }
  // 第 33 次切换的 touch 使 sessionStates 达到 33 → 淘汰最旧的 s1。
  assert.equal(purgeLog.filter(e => e.reason === 'evict').length, 1);
  assert.deepEqual(purgeLog[0], { id: 's1', reason: 'evict' });
  assert.equal(Object.keys(sessionStates).length, 32, '重对象 buffer 必须有界（≤32）');
  assert.equal(sessionStates.s1, undefined, '最旧空闲 buffer 必须被淘汰');
  assert.equal(sessionStates.s2.composerDraft, 'draft-2', '存活 buffer 草稿原样');

  // 重访问 s1：switchActiveTo 重建 buffer（非 fresh）→ 暂存草稿回填。
  api.switchActiveTo('s1', null);
  assert.equal(state.composerDraft, 'draft-1', '淘汰时的未发送草稿必须经侧表恢复');
  assert.equal(sessionStates.s1.composerDraft, 'draft-1');
  // 重水化语义：新 buffer 不携带旧 messages（由 load_session 慢路径重建）。
  assert.equal(state.messages.length, 0);
});

test('tauri 淘汰保护谓词：busy / queued / remote 不回收', () => {
  const { api, state, sessionStates } = loadTauriSessionsFeature();
  api.switchActiveTo('s1', null);
  state.composerDraft = 'd1';
  state.busy = true; // 回合进行中离开：saveWorkingSetTo 把 busy 快照进 s1 buffer
  for (let i = 2; i <= 34; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `d${i}`;
  }
  assert.notEqual(sessionStates.s1, undefined, 'busy buffer 不得被容量淘汰');
  assert.equal(sessionStates.s1.busy, true);
  assert.equal(Object.keys(sessionStates).length, 32);
});

test('ta purgeSessionBuffer：真实会话删除作废暂存草稿，不回流同 id 重建 buffer', () => {
  const { api, state, sessionStates, purgeLog } = loadTauriSessionsFeature();
  for (let i = 1; i <= 33; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `draft-${i}`;
  }
  assert.equal(sessionStates.s1, undefined, '前置：s1 已被淘汰且草稿已暂存');
  assert.deepEqual(purgeLog[0], { id: 's1', reason: 'evict' });

  api.purgeSessionBuffer('s1');
  assert.deepEqual(purgeLog[purgeLog.length - 1], { id: 's1', reason: 'delete' },
    '真实删除必须以 delete reason 通知宿主（scene 键清理的依据）');
  const rebuilt = api.getBuffer('s1');
  assert.equal(rebuilt.composerDraft, '', '已删除会话的暂存草稿不得回流');

  // 对照：未删除的 s2 暂存草稿仍可回填。
  api.switchActiveTo('s2', null);
  assert.equal(state.composerDraft, 'draft-2');
});

test('tauri 暂存草稿侧表有界（256）：远超上限的淘汰逐出最旧暂存', () => {
  const { api, state } = loadTauriSessionsFeature();
  const total = 32 + 257; // 289 个会话 → 257 次淘汰 → 暂存表 257 条超限逐出 s1
  for (let i = 1; i <= total; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `draft-${i}`;
  }
  // 探查本身会触发新一轮淘汰+暂存（挤掉当时的表头），所以只断言两个
  // 稳定不变量：最早的暂存（s1）已被逐出；近期暂存（第 257 个被淘汰的
  // s257）仍在表内。二者都不受探查顺序影响。
  assert.equal(api.getBuffer('s1').composerDraft, '', '最早的暂存草稿被侧表上限逐出');
  assert.equal(api.getBuffer('s257').composerDraft, 'draft-257',
    '近期暂存草稿仍在侧表内（侧表非无限，但容量远大于 buffer 上限）');
});

// ── P2：scene 缓存键语义（源码契约 + tauri hook reason）────────────

test('scene 事件 localStorage 键只在真实会话删除时清理（tauri hook 契约）', () => {
  const tauriBridge = fs.readFileSync(path.join(bridgeDir, '..', 'bridge.js'), 'utf8');
  // onSessionBufferPurged 必须按 reason 区分，仅 delete 清理 scene 缓存键。
  assert.match(tauriBridge, /onSessionBufferPurged: function \(id, reason\)/,
    'tauri bridge hook 必须接收淘汰原因');
  const sceneRemoves = tauriBridge.match(/removeItem\(PINVOU_SCENE_EVENTS_STORAGE_PREFIX \+ id\)/g) || [];
  assert.equal(sceneRemoves.length, 1, 'tauri bridge 只应有一处 scene 键清理（hook 内）');
  const hookBody = tauriBridge.slice(
    tauriBridge.indexOf('onSessionBufferPurged: function (id, reason)'),
    tauriBridge.indexOf('onSessionBufferPurged: function (id, reason)') + 900);
  assert.match(hookBody, /reason === "delete"[\s\S]{0,400}removeItem\(PINVOU_SCENE_EVENTS_STORAGE_PREFIX/,
    'scene 键清理必须被 reason === "delete" 门控');

  // web 桥：两处 LRU 淘汰循环不得清理 scene 键；清理只存在于 purgeSessionBuffer。
  const webBridge = read('bridge.js');
  const webSceneRemoves = webBridge.match(/removeItem\(PINVOU_SCENE_EVENTS_STORAGE_PREFIX \+ id\)/g) || [];
  assert.equal(webSceneRemoves.length, 1, 'web bridge 的 scene 键清理只应在 purgeSessionBuffer（真实删除）');
  const purgeAt = webBridge.indexOf('function purgeSessionBuffer');
  const pruneAt = webBridge.indexOf('function pruneSessionBuffers');
  assert.ok(purgeAt > 0 && pruneAt > 0);
  assert.ok(webSceneRemoves[0] && webBridge.indexOf(webSceneRemoves[0]) > purgeAt,
    'web scene 键清理必须位于 purgeSessionBuffer 内');
});

// ── web 桥：整桥装载驱动公开 API ──────────────────────────────────

function bootWebBridge() {
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
  const listeners = {};
  const deferreds = {};
  const handlers = {};
  const calls = { invoke: [], chunkLoads: [] };
  const windowObject = {
    PinvouPlatform: {
      kind: 'web',
      isWeb: true,
      capabilities: {},
      can: () => false,
      canInvoke: () => false,
      areInvokeCapabilitiesReady: () => true,
    },
    __TAURI__: {
      core: { invoke: (name, args) => {
        calls.invoke.push(name);
        if (handlers[name]) return Promise.resolve(handlers[name](args || {}));
        const d = deferreds[name];
        if (d && d.promise) return d.promise;
        return Promise.resolve(null);
      } },
      event: { listen: async (name, fn) => { (listeners[name] = listeners[name] || []).push(fn); return function () {}; } },
      dialog: { open: async () => null },
    },
    location: { search: '', href: 'https://example.test/pinvou3/remote/' },
    localStorage,
    crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
    performance: { now: () => 0 },
    atob: value => Buffer.from(String(value), 'base64').toString('binary'),
    btoa: value => Buffer.from(String(value), 'binary').toString('base64'),
    addEventListener() {},
    removeEventListener() {},
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
  vm.runInContext(read('bridge.js'), context, { filename: 'platform/web/bridge.js' });
  const flat = windowObject.TauriBridge;
  // 冷切换走 loadSessionForClient 的 chunk 协议：单块 eof 返回。
  handlers.web_access_load_session_chunk = args => {
    calls.chunkLoads.push(args.id);
    const saved = {
      metadata: { id: args.id, title: `会话 ${args.id}`, message_count: 1 },
      messages: [{ role: 'user', content: `hello ${args.id}` }],
      transcript_revision: 1,
      artifacts: [],
    };
    const encoded = Buffer.from(JSON.stringify(saved), 'utf8');
    return {
      download_id: args.downloadId || args.requestedDownloadId || `dl-${args.id}`,
      offset: Number(args.offset || 0),
      total: encoded.length,
      data_base64: encoded.subarray(Number(args.offset || 0)).toString('base64'),
      eof: true,
    };
  };
  return {
    flat, storage, calls, handlers, deferreds,
    view() { return flat.getState(); },
  };
}

// ── P1（web）：33+ 会话淘汰后草稿仍恢复 ────────────────────────────

test('web 34 会话 LRU：淘汰 s1 后重访问，草稿经侧表回填且走磁盘重水化', async () => {
  const rt = bootWebBridge();
  // s1 冷加载（慢路径建立 loadedFromDisk buffer）并留下未发送草稿。
  assert.equal(await rt.flat.switchToSession('s1'), true);
  rt.flat.setComposerDraft('web-unsent-draft');
  // 再切换 33 个会话：第 34 个会话建立时容量淘汰 s1（最旧空闲）。
  for (let i = 2; i <= 34; i++) {
    assert.equal(await rt.flat.switchToSession(`s${i}`), true);
  }
  const loadsBefore = rt.calls.chunkLoads.filter(id => id === 's1').length;
  assert.equal(loadsBefore, 1, '前置：s1 此前只冷加载过一次');
  // 切回 s1：buffer 已被淘汰 → 必须重新走 chunk 重水化（证明淘汰真实发生）。
  assert.equal(await rt.flat.switchToSession('s1'), true);
  const loadsAfter = rt.calls.chunkLoads.filter(id => id === 's1').length;
  assert.equal(loadsAfter, 2, 's1 被淘汰后重访问必须重新水化（重对象确已释放）');
  assert.equal(rt.flat.getComposerDraft(), 'web-unsent-draft',
    '淘汰时的未发送草稿必须经侧表恢复');
});

// ── P2（web）：保存失败 + 淘汰 + 重水化 ────────────────────────────

test('web scene sidecar 保存失败 + 容量淘汰后，localStorage 缓存仍可恢复', async () => {
  const rt = bootWebBridge();
  const key = 'pinvou_scene_events_v1:s1';
  // 模拟「sidecar 尚无数据、后端保存失败」：get 返回空数组、save 拒绝，
  // localStorage 缓存是唯一副本（savePinvouSceneEventsForSession 的后端
  // 失败被有意吞掉，留下缓存）。
  rt.handlers.get_session_pinvou_scene_events = () => [];
  rt.handlers.save_session_pinvou_scene_events = () => Promise.reject(new Error('offline'));
  rt.storage.set(key, JSON.stringify([{ pos: 0, scene: 'work:document-writing' }]));

  assert.equal(await rt.flat.switchToSession('s1'), true);
  assert.equal(rt.view().pinvouSceneEvents.length, 1,
    'sidecar 空 + 保存失败时必须由 localStorage 缓存兜底恢复');
  assert.equal(rt.storage.has(key), true);

  for (let i = 2; i <= 34; i++) {
    assert.equal(await rt.flat.switchToSession(`s${i}`), true);
  }
  // 容量淘汰不得清理恢复副本。
  assert.equal(rt.storage.has(key), true,
    'LRU 容量淘汰不得删除 scene 事件的唯一离线恢复副本');
  // 淘汰后重水化仍然恢复。
  assert.equal(await rt.flat.switchToSession('s1'), true);
  assert.equal(rt.view().pinvouSceneEvents.length, 1,
    '淘汰 + 重水化后 scene 映射必须从缓存恢复');

  // 对照：真实会话删除才清理缓存键。
  rt.handlers.delete_session = () => ({});
  assert.equal(await rt.flat.deleteSession('s1'), true);
  assert.equal(rt.storage.has(key), false,
    '真实会话删除必须清理 scene 缓存键（防无界累积）');
});
