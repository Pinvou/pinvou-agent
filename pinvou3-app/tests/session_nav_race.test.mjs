/**
 * 会话管理导航竞态回归测试（PR #257 三审）：ensureSession / archiveSession
 * 的「null → null」导航窗口——用户再进草稿（enterDraft）时 activeSessionId
 * 保持 null 但 sessionSwitchRequestToken 已前移，仅判 activeSessionId 的
 * 守卫拦不住在途异步操作劫持新草稿。修复：请求开始时捕获导航 token，
 * 写入/回滚前连同 activeSessionId 一起校验。
 *
 * 覆盖 tauri 桥（platform/tauri/bridge/sessions.js，factory 注入依赖）；
 * web 桥镜像守卫由 session_nav_race_web.test.mjs 锁定。此前 sessions.js
 * 因「依赖太重」只靠代码审查（见 memory_personas_race.test.mjs 头注释），
 * 三审后注入面已验证可单测，补上可执行回归。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');

/** vm 装载 sessions.js factory，注入最小依赖面。 */
function loadSessionsFeature(overrides) {
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
    modeDefaults: { work: 'yolo', code: 'yolo' },
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
    // eslint-disable-next-line no-unused-vars -- test stub / deferred assignment kept
    invoke(name, args) {
      calls.invoke.push(name);
      if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
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
    api, state, sessionStates, calls,
    defer(name) {
      // 每次创建全新对象：同一 invoke 名的第二次 defer 不得覆盖第一次的 resolve。
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
  };
}

// ── ensureSession：再进草稿不劫持 ──────────────────────────

test('ensureSession：create_session 等待期间再进草稿 → 不物化、返回 null', async () => {
  const rt = loadSessionsFeature();
  const create = rt.defer('create_session');
  const p = rt.api.ensureSession();
  rt.api.enterDraft(); // 用户在慢 create_session 期间点了「新建对话」（再进草稿）
  create.resolve({ id: 'chat-new' });
  assert.equal(await p, null, '导航 token 已前移，物化必须中止');
  assert.equal(rt.state.activeSessionId, null, '不得劫持用户的新草稿');
  assert.ok(rt.sessionStates['chat-new'], '新会话仍应登记为后台 buffer 等待下次切换');
  assert.equal(rt.sessionStates['chat-new'].loadedFromDisk, true);
});

test('ensureSession：无导航 → 正常物化返回 meta.id', async () => {
  const rt = loadSessionsFeature();
  const create = rt.defer('create_session');
  const p = rt.api.ensureSession();
  create.resolve({ id: 'chat-ok' });
  assert.equal(await p, 'chat-ok');
  assert.equal(rt.state.activeSessionId, 'chat-ok');
});

test('ensureSession：尾部 await 期间再进草稿 → 返回 null 不漂移', async () => {
  const rt = loadSessionsFeature();
  const create = rt.defer('create_session');
  const list = rt.defer('list_sessions'); // 挂住尾部链，制造真实 await 窗口
  const p = rt.api.ensureSession();
  create.resolve({ id: 'chat-tail' });
  await new Promise(r => { setTimeout(r, 0); }); // materialization advances to the trailing refreshHistoryList and suspends
  rt.api.enterDraft(); // 尾部 await 期间再进草稿：activeSessionId 被 enterDraft 置 null、token 前移
  list.resolve([]);
  assert.equal(await p, null, '尾部窗口的再进草稿同样必须中止物化');
  assert.equal(rt.state.activeSessionId, null);
});

test('ensureSession：create_session 等待期间切到既有会话 → 返回 null（既有行为保持）', async () => {
  const rt = loadSessionsFeature();
  const create = rt.defer('create_session');
  const p = rt.api.ensureSession();
  rt.state.activeSessionId = 'chat-b'; // 用户直接切到既有会话 B
  create.resolve({ id: 'chat-new' });
  assert.equal(await p, null);
  assert.equal(rt.state.activeSessionId, 'chat-b', '不得改写已激活的会话 B');
  assert.ok(rt.sessionStates['chat-new'], '新会话登记为后台 buffer');
});

// ── archiveSession：失败回滚不劫持新草稿 ──────────────────────────

test('archiveSession（普通会话）：归档失败但等待期间再进草稿 → 不回滚 active', async () => {
  const rt = loadSessionsFeature();
  rt.state.sessions = [{ id: 'chat-a', title: 'A' }];
  rt.state.activeSessionId = 'chat-a';
  const set = rt.defer('set_session_archived');
  const p = rt.api.archiveSession('chat-a');
  await new Promise(r => { setTimeout(r, 0); }); // advance until leaveSessionView has run (active already set to null)
  assert.equal(rt.state.activeSessionId, null);
  rt.api.enterDraft(); // 归档等待期间用户再进草稿：token 前移、active 仍为 null
  set.reject(new Error('backend down'));
  assert.equal(await p, false);
  assert.equal(rt.state.activeSessionId, null, '失败回滚不得把用户拽回归档会话（劫持新草稿）');
  assert.equal(rt.state.sessions.length, 1, '会话列表仍按失败语义回滚恢复');
});

test('archiveSession（普通会话）：归档失败且无导航 → 回滚 active（既有行为保持）', async () => {
  const rt = loadSessionsFeature();
  rt.state.sessions = [{ id: 'chat-a', title: 'A' }];
  rt.state.activeSessionId = 'chat-a';
  const set = rt.defer('set_session_archived');
  const p = rt.api.archiveSession('chat-a');
  await new Promise(r => { setTimeout(r, 0); });
  set.reject(new Error('backend down'));
  assert.equal(await p, false);
  assert.equal(rt.state.activeSessionId, 'chat-a', '无新导航时仍应回滚恢复 active');
});

test('archiveSession（scheduled run）：归档失败但等待期间再进草稿 → 不回滚 active/context', async () => {
  const rt = loadSessionsFeature();
  rt.state.activeSessionId = 'sched-run-1';
  rt.state.scheduledRunContext = { sessionId: 'sched-run-1' };
  rt.state.scheduledTaskRecentRuns = [{ sessionId: 'sched-run-1', runId: 'run-1' }];
  const set = rt.defer('set_session_archived');
  const p = rt.api.archiveSession('sched-run-1');
  await new Promise(r => { setTimeout(r, 0); });
  assert.equal(rt.state.activeSessionId, null);
  rt.api.enterDraft(); // 等待期间再进草稿
  set.reject(new Error('backend down'));
  assert.equal(await p, false);
  assert.equal(rt.state.activeSessionId, null, '失败回滚不得劫持新草稿');
  assert.equal(rt.state.scheduledRunContext, null, 'context 也不得成对回滚');
});

test('archiveSession（scheduled run）：归档失败且无导航 → active/context 成对回滚（既有行为保持）', async () => {
  const rt = loadSessionsFeature();
  rt.state.activeSessionId = 'sched-run-1';
  rt.state.scheduledRunContext = { sessionId: 'sched-run-1' };
  rt.state.scheduledTaskRecentRuns = [{ sessionId: 'sched-run-1', runId: 'run-1' }];
  const set = rt.defer('set_session_archived');
  const p = rt.api.archiveSession('sched-run-1');
  await new Promise(r => { setTimeout(r, 0); });
  set.reject(new Error('backend down'));
  assert.equal(await p, false);
  assert.equal(rt.state.activeSessionId, 'sched-run-1', '无新导航时成对回滚');
  assert.deepEqual(rt.state.scheduledRunContext, { sessionId: 'sched-run-1' });
});
