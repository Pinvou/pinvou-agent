/**
 * Session fork bridge contract (docs/fork-session-plan.md §6.2-1/2):
 *
 * 1. `forkSession` must invoke the `fork_session` command with the exact
 *    argument shape the Rust command declares (camelCase keys, null for the
 *    optional v2 parameters), refresh the history list once the command
 *    resolves, and resolve to the backend result so main.jsx can perform the
 *    switch (D8: the switch itself belongs to the React layer, which owns the
 *    browser-UI transition).
 * 2. In-flight dedupe: a repeat call for the same source while a fork is
 *    running resolves to null and must NOT issue a second invoke — the dialog
 *    disables itself while busy, and this is the second, independent line.
 * 3. Failure propagates as a rejection (the caller toasts; the bridge never
 *    swallows a fork error into a chat system item because the fork source
 *    may not be the active buffer).
 *
 * Loads the tauri sessions.js bridge factory the same way as
 * session_buffer_eviction.test.mjs.
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');

function loadTauriSessionsFeature(invokeImpl) {
  const root = {};
  const src = fs.readFileSync(path.join(bridgeDir, 'sessions.js'), 'utf8');
  const sharedHelpers = fs.readFileSync(path.join(bridgeDir, '..', '..', '..', 'shared', 'bridge-shared-helpers.js'), 'utf8');
  vm.runInNewContext(sharedHelpers + '\n' + src, { window: root, globalThis: root, setTimeout, clearTimeout });
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
  };
  const calls = { invoke: [] };
  const api = factory({
    state,
    sessionStates: {},
    notify() {},
    listen: null,
    bt(key) { return key; },
    onSessionBufferPurged() {},
    addSystemItem(text) { state.chatItems.push({ type: 'system', text }); },
    addChatItem(item) { state.chatItems.push(item); },
    timeStr() { return ''; },
    invoke(name, args) {
      calls.invoke.push({ name, args });
      return invokeImpl ? invokeImpl(name, args) : Promise.resolve({});
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
  });
  return { api, state, calls };
}

test('forkSession invokes fork_session with the command contract and refreshes the list', async () => {
  const backendResult = {
    session_id: 'newsess1234',
    title: '修 bug（分叉2）',
    message_count: 7,
    root_map: [['/a/bugfix', '/a/bugfix-fork-1234']],
  };
  const { api, calls } = loadTauriSessionsFeature((name) => {
    if (name === 'fork_session') return Promise.resolve(backendResult);
    if (name === 'list_sessions') return Promise.resolve([]);
    if (name === 'list_archived_sessions') return Promise.resolve([]);
    return Promise.resolve({});
  });

  const result = await api.forkSession('srcsess9999', undefined, ['/a/bugfix'], 'ja');

  // v1 UI calls with keepTurns/isolateRoots/locale; optional params must land
  // as null (not undefined) for the Tauri IPC deserializer. JSON comparison:
  // the args object is built inside the vm realm (different prototype).
  const forkCall = calls.invoke.find(call => call.name === 'fork_session');
  assert.ok(forkCall, 'fork_session must be invoked');
  assert.equal(
    JSON.stringify(forkCall.args),
    JSON.stringify({ sessionId: 'srcsess9999', keepTurns: null, isolateRoots: ['/a/bugfix'], locale: 'ja' }),
  );
  // The history list is refreshed after the command resolves (ordering
  // matters: the switch reads the fresh list).
  const names = calls.invoke.map(call => call.name);
  assert.ok(names.indexOf('list_sessions') > names.indexOf('fork_session'), 'refresh must follow the fork');
  // The backend result is returned verbatim for the caller's switch + toast.
  assert.equal(JSON.stringify(result), JSON.stringify(backendResult));
});

test('a repeat call while a fork of the same session is in flight resolves to null without invoking', async () => {
  let releaseFork;
  const gate = new Promise(resolve => { releaseFork = resolve; });
  const { api, calls } = loadTauriSessionsFeature((name) => {
    if (name === 'fork_session') return gate.then(() => ({}));
    return Promise.resolve([]);
  });

  const first = api.forkSession('srcsess1', null, [], 'zh');
  const second = await api.forkSession('srcsess1', null, [], 'zh');
  assert.equal(second, null, 'the in-flight duplicate must resolve to null, not double-fork');
  releaseFork();
  await first;
  assert.equal(
    calls.invoke.filter(call => call.name === 'fork_session').length,
    1,
    'exactly one fork_session invoke may happen for a concurrent pair',
  );
  // A fork of a DIFFERENT session is not blocked by the in-flight one.
  const other = await api.forkSession('srcsess2', null, [], 'zh');
  assert.ok(other, 'a different source is not deduped');
});

test('fork failures propagate as rejections instead of being swallowed', async () => {
  const { api } = loadTauriSessionsFeature((name) => {
    if (name === 'fork_session') return Promise.reject(new Error('会话正在执行，请先停止当前任务再分叉'));
    return Promise.resolve([]);
  });
  await assert.rejects(
    () => api.forkSession('srcsess1', null, [], 'zh'),
    /会话正在执行/,
    'the caller (main.jsx) owns toasting the failure',
  );
});
