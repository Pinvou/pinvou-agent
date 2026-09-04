/**
 * 普通聊天草稿态工作目录选择（sessions bridge）逻辑测试：
 *   - setDraftWorkspace 仅草稿态生效，enterDraft 复位为 null；
 *   - ensureSession 物化时 create_session 载荷携带 workspacePath
 *     （未选择时显式 null = 后端现状），物化成功后清除选择；
 *   - create_session 失败保留选择以便重试；
 *   - pickDraftWorkspace 经注入的 dialogOpen 选目录，选中后记入最近列表
 *     （与 src/shared/workspace-recents.js 同 key 同语义的桥侧镜像），
 *     用户取消返回 null 且不改选择。
 * harness 复刻 session_nav_race.test.mjs 的 vm 注入面（factory 最小依赖）。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');
const RECENTS_KEY = 'pinvou_codex_recent_workspaces';

/** vm 装载 sessions.js factory，注入最小依赖面 + 内存 localStorage。 */
function loadSessionsFeature(overrides) {
  const storage = new Map();
  const localStorage = {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
    removeItem(key) { storage.delete(key); },
  };
  const root = { __PINVOU_SHARED_I18N__: {} };
  const src = fs.readFileSync(path.join(bridgeDir, 'sessions.js'), 'utf8');
  vm.runInNewContext(src, { window: root, globalThis: root, localStorage, setTimeout, clearTimeout });
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
    draftWorkspacePath: null,
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
  const deferreds = {};
  const calls = { invoke: [] };
  const api = factory(Object.assign({
    state,
    sessionStates,
    notify() { calls.notify = (calls.notify || 0) + 1; },
    listen: null,
    bt(key) { return key; },
    addSystemItem(text) { state.chatItems.push({ type: 'system', text, id: 'sys-' + (state.chatItems.length + 1) }); },
    addChatItem(item) { item.id = item.id || ('item-' + (state.chatItems.length + 1)); state.chatItems.push(item); },
    timeStr() { return ''; },
    invoke(name, args) {
      calls.invoke.push([name, args]);
      if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
      if (name === 'create_session') return Promise.resolve({ id: 'chat-new' });
      if (name === 'list_sessions' || name === 'list_archived_sessions') return Promise.resolve([]);
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
    turnUsageDirty: false,
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
  }, overrides || {}));
  return {
    api, state, sessionStates, calls, storage,
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
    createSessionArgs() {
      // invoke 入参对象产自 vm realm，JSON 归一化后再比较（跨域原型不等）。
      return calls.invoke
        .filter(call => call[0] === 'create_session')
        .map(call => JSON.parse(JSON.stringify(call[1])));
    },
  };
}

// ── setDraftWorkspace：仅草稿态生效 ──────────────────────────

test('setDraftWorkspace：草稿态更新 state 并 notify；null 回默认', () => {
  const rt = loadSessionsFeature();
  rt.api.setDraftWorkspace('/work/project');
  assert.equal(rt.state.draftWorkspacePath, '/work/project');
  assert.equal(rt.calls.notify, 1);
  rt.api.setDraftWorkspace(null);
  assert.equal(rt.state.draftWorkspacePath, null);
});

test('setDraftWorkspace：已有 active 会话时忽略（不物化语义之外不得改写）', () => {
  const rt = loadSessionsFeature();
  rt.state.activeSessionId = 'chat-a';
  rt.api.setDraftWorkspace('/work/project');
  assert.equal(rt.state.draftWorkspacePath, null);
  assert.equal(rt.calls.notify, undefined, '非草稿态不得 notify');
});

// ── ensureSession：workspacePath 载荷与清除时机 ──────────────────────────

test('ensureSession：草稿选择随 create_session 载荷下发，物化成功后清除', async () => {
  const rt = loadSessionsFeature();
  rt.api.setDraftWorkspace('/work/project');
  const id = await rt.api.ensureSession();
  assert.equal(id, 'chat-new');
  assert.deepEqual(rt.createSessionArgs(), [{ workspacePath: '/work/project' }]);
  assert.equal(rt.state.draftWorkspacePath, null, '物化成功后草稿选择必须清除');
});

test('ensureSession：未选择工作区时载荷显式为 null（后端现状：会话私有目录）', async () => {
  const rt = loadSessionsFeature();
  const id = await rt.api.ensureSession();
  assert.equal(id, 'chat-new');
  assert.deepEqual(rt.createSessionArgs(), [{ workspacePath: null }]);
});

test('ensureSession：create_session 失败保留草稿选择以便重试', async () => {
  const rt = loadSessionsFeature({
    invoke(name) {
      if (name === 'create_session') return Promise.reject(new Error('backend down'));
      return Promise.resolve(name === 'list_sessions' || name === 'list_archived_sessions' ? [] : {});
    },
  });
  rt.api.setDraftWorkspace('/work/project');
  const id = await rt.api.ensureSession();
  assert.equal(id, null, '创建失败返回 null');
  assert.equal(rt.state.draftWorkspacePath, '/work/project', '失败路径必须保留选择');
});

test('enterDraft：复位草稿工作区选择（含已在干净草稿态的提前返回分支）', () => {
  const rt = loadSessionsFeature();
  rt.api.setDraftWorkspace('/work/project');
  rt.api.enterDraft();
  assert.equal(rt.state.draftWorkspacePath, null);
  // 干净草稿态再点「新建对话」（提前返回分支）同样复位。
  rt.api.setDraftWorkspace('/work/again');
  rt.api.enterDraft();
  assert.equal(rt.state.draftWorkspacePath, null);
});

// ── ensureSession：多智能体开关落盘失败的草稿回退 ──────────────────────────

test('ensureSession：多智能体开关失败回退草稿保留目录绑定与开关意图', async () => {
  const invokeLog = [];
  const rt = loadSessionsFeature({
    invoke(name, args) {
      invokeLog.push([name, args]);
      if (name === 'set_multi_agent_mode') return Promise.reject(new Error('persist down'));
      if (name === 'create_session') return Promise.resolve({ id: 'chat-new' });
      return Promise.resolve(name === 'list_sessions' || name === 'list_archived_sessions' ? [] : {});
    },
  });
  rt.api.setDraftWorkspace('/work/project');
  rt.state.pendingDraftMultiAgent = true;
  const id = await rt.api.ensureSession();
  assert.equal(id, null, '开关落盘失败必须中止物化');
  // 空会话已回滚删除。
  assert.deepEqual(
    invokeLog.filter(call => call[0] === 'delete_session').map(call => JSON.parse(JSON.stringify(call[1]))),
    [{ id: 'chat-new' }],
  );
  // 回退草稿保留失败前的寄存意图：目录绑定不丢，重试不必重选。
  assert.equal(rt.state.activeSessionId, null);
  assert.equal(rt.state.draftWorkspacePath, '/work/project');
  assert.equal(rt.state.pendingDraftMultiAgent, true);
  assert.equal(rt.state.modeState.multiAgent, true);
});

test('ensureSession：多智能体开关失败回退草稿同时保留显式 mode 暂存', async () => {
  const rt = loadSessionsFeature({
    invoke(name) {
      if (name === 'set_multi_agent_mode') return Promise.reject(new Error('persist down'));
      if (name === 'create_session') return Promise.resolve({ id: 'chat-new' });
      return Promise.resolve(name === 'list_sessions' || name === 'list_archived_sessions' ? [] : {});
    },
  });
  rt.api.setDraftWorkspace('/work/project');
  rt.state.pendingDraftMode = 'plan';
  rt.state.pendingDraftMultiAgent = true;
  const id = await rt.api.ensureSession();
  assert.equal(id, null);
  assert.equal(rt.state.pendingDraftMode, 'plan', '显式 mode 暂存不得丢失');
  assert.equal(rt.state.modeState.mode, 'plan', 'mode 显示按暂存值恢复');
  assert.equal(rt.state.draftWorkspacePath, '/work/project');
});

// ── pickDraftWorkspace：系统目录对话框 ──────────────────────────

test('pickDraftWorkspace：选中后写回草稿选择并记入最近列表', async () => {
  const dialogCalls = [];
  const rt = loadSessionsFeature({
    dialogOpen: async options => { dialogCalls.push(options); return '/work/picked'; },
  });
  const picked = await rt.api.pickDraftWorkspace();
  assert.equal(picked, '/work/picked');
  assert.equal(rt.state.draftWorkspacePath, '/work/picked');
  // dialogOpen 的入参来自 vm realm，跨域对象用 JSON 比较而非 deepEqual。
  assert.deepEqual(dialogCalls.map(options => JSON.parse(JSON.stringify(options))), [{ directory: true, multiple: false, title: 'pickFolderTitle' }]);
  // 最近列表与共享模块同 key，选中条目置顶。
  assert.deepEqual(JSON.parse(rt.storage.get(RECENTS_KEY)), ['/work/picked']);
});

test('pickDraftWorkspace：最近列表去重置顶、6 条上限（与共享模块同语义）', async () => {
  let next = null;
  const rt = loadSessionsFeature({
    dialogOpen: async () => next,
  });
  for (const path of ['/1', '/2', '/3', '/4', '/5', '/6']) {
    next = path;
    await rt.api.pickDraftWorkspace();
  }
  next = '/3'; // 重复选择去重并置顶
  await rt.api.pickDraftWorkspace();
  next = '/7';
  await rt.api.pickDraftWorkspace();
  assert.deepEqual(JSON.parse(rt.storage.get(RECENTS_KEY)), ['/7', '/3', '/6', '/5', '/4', '/2']);
});

test('pickDraftWorkspace：用户取消返回 null，不改变现有选择', async () => {
  const rt = loadSessionsFeature({ dialogOpen: async () => null });
  rt.api.setDraftWorkspace('/work/keep');
  const picked = await rt.api.pickDraftWorkspace();
  assert.equal(picked, null);
  assert.equal(rt.state.draftWorkspacePath, '/work/keep', '取消不得清空已有选择');
  assert.equal(rt.storage.get(RECENTS_KEY), undefined, '取消不得写最近列表');
});

test('pickDraftWorkspace：对话框不可用或非草稿态返回 null', async () => {
  const rt = loadSessionsFeature(); // 未注入 dialogOpen
  assert.equal(await rt.api.pickDraftWorkspace(), null);
  const rt2 = loadSessionsFeature({ dialogOpen: async () => '/work/picked' });
  rt2.state.activeSessionId = 'chat-a';
  assert.equal(await rt2.api.pickDraftWorkspace(), null);
  assert.equal(rt2.state.draftWorkspacePath, null);
});
