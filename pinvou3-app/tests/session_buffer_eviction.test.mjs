/**
 * Session buffer LRU eviction regression tests (PR #339, round-5 P1/P2):
 *
 * P1 — The all-session LRU evicting an idle buffer used to drop its
 * composerDraft with it. Disk transcripts hold committed content only and
 * drafts are never persisted (both tauri and web), so eviction turned a
 * recoverable draft into permanent loss. Fix: before eviction the
 * non-rehydratable lightweight draft moves to an evictedSessionDrafts side
 * table (bounded at 256 entries / 1M chars each) and every buffer-rebuild
 * path restores it; when a bound would be exceeded the eviction is refused
 * so the draft stays in the live buffer; real session deletion
 * (purgeSessionBuffer) invalidates stashed drafts so they never flow back.
 *
 * P2 — LRU eviction used to clean the scene-events localStorage cache key
 * as if it were session deletion. That cache is the only recovery copy
 * when the sidecar save fails or we are offline
 * (savePinvouSceneEventsForSession intentionally swallows backend
 * failures; syncPinvouSceneEventsForSession replays from it). Fix:
 * capacity eviction (reason="evict") keeps the key; only real session
 * deletion (reason="delete") cleans it.
 *
 * The tauri side loads the sessions.js factory (same as
 * session_nav_race.test.mjs); the web side boots the whole bridge in a vm
 * (same as session_nav_race_web.test.mjs) and drives the public API
 * switchToSession/setComposerDraft/deleteSession through production paths.
 *
 * Follow-up rounds added:
 * - fresh materialization (ensureSession/persona card) must mark the empty
 *   buffer loadedFromDisk, otherwise switching back with a draft falls
 *   into the slow path and the freshBuffer() replacement drops the draft;
 * - personaPlaceholderTitles is session metadata: kept on capacity
 *   eviction, cleaned only on real deletion;
 * - eviction must never lose a draft: over-cap and capacity-boundary
 *   drafts are preserved (stash overflow refuses eviction instead of
 *   silently discarding user input).
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

// ── tauri sessions.js factory boot ───────────────────────────────────

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
    invoke(name) {
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

// ── P1: 33-session eviction restores drafts via the side table (tauri) ──

test('tauri 33-session LRU evicts the oldest idle buffer, restores its draft, keeps buffer count bounded', () => {
  const { api, state, sessionStates, purgeLog } = loadTauriSessionsFeature();
  const total = 33;
  for (let i = 1; i <= total; i++) {
    api.switchActiveTo(`s${i}`, null);
    // Type after switching in: what saveWorkingSetTo snapshots when leaving
    // s_i is draft-i (production semantics).
    state.composerDraft = `draft-${i}`;
    state.messages = [{ role: 'user', content: `m${i}` }];
  }
  // The 33rd switch's touch brings sessionStates to 33 → evicts oldest s1.
  assert.equal(purgeLog.filter(e => e.reason === 'evict').length, 1);
  assert.deepEqual(purgeLog[0], { id: 's1', reason: 'evict' });
  assert.equal(Object.keys(sessionStates).length, 32, 'heavy buffers must stay bounded (≤32)');
  assert.equal(sessionStates.s1, undefined, 'the oldest idle buffer must be evicted');
  assert.equal(sessionStates.s2.composerDraft, 'draft-2', 'surviving buffers keep their drafts');

  // Revisit s1: switchActiveTo rebuilds the buffer (non-fresh) → the
  // stashed draft is restored.
  api.switchActiveTo('s1', null);
  assert.equal(state.composerDraft, 'draft-1', 'the unsent draft at eviction time must be restored via the side table');
  assert.equal(sessionStates.s1.composerDraft, 'draft-1');
  // Rehydration semantics: the new buffer carries no old messages (the
  // load_session slow path rebuilds them).
  assert.equal(state.messages.length, 0);
});

test('tauri eviction protection predicates: busy / queued / remote buffers are never reclaimed', () => {
  const { api, state, sessionStates } = loadTauriSessionsFeature();
  api.switchActiveTo('s1', null);
  state.composerDraft = 'd1';
  state.busy = true; // leaving mid-turn: saveWorkingSetTo snapshots busy into s1's buffer
  for (let i = 2; i <= 34; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `d${i}`;
  }
  assert.notEqual(sessionStates.s1, undefined, 'a busy buffer must not be evicted for capacity');
  assert.equal(sessionStates.s1.busy, true);
  assert.equal(Object.keys(sessionStates).length, 32);
});

test('tauri purgeSessionBuffer: real session deletion invalidates stashed drafts so they never flow back', () => {
  const { api, state, sessionStates, purgeLog } = loadTauriSessionsFeature();
  for (let i = 1; i <= 33; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `draft-${i}`;
  }
  assert.equal(sessionStates.s1, undefined, 'precondition: s1 already evicted and its draft stashed');
  assert.deepEqual(purgeLog[0], { id: 's1', reason: 'evict' });

  api.purgeSessionBuffer('s1');
  assert.deepEqual(purgeLog[purgeLog.length - 1], { id: 's1', reason: 'delete' },
    'real deletion must notify the host with the delete reason (the scene-key cleanup depends on it)');
  const rebuilt = api.getBuffer('s1');
  assert.equal(rebuilt.composerDraft, '', 'a deleted session\'s stashed draft must not flow back');

  // Control: s2's stashed draft (not deleted) is still restorable.
  api.switchActiveTo('s2', null);
  assert.equal(state.composerDraft, 'draft-2');
});

test('tauri oversized drafts never lost: eviction is refused when the stash cannot retain the draft', () => {
  const { api, state, sessionStates } = loadTauriSessionsFeature();
  api.switchActiveTo('s1', null);
  // A transport-level write can bypass the composer input cap, producing an
  // oversized draft (over the 1M-char stash bound).
  state.composerDraft = 'x'.repeat(1000001);
  for (let i = 2; i <= 33; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `d${i}`;
  }
  // s1 is oldest and idle, but its oversized draft cannot be safely
  // stashed: the eviction must be refused so the draft stays in the live
  // buffer (never silently discarded). s2 becomes the oldest eligible
  // buffer and is evicted instead.
  assert.notEqual(sessionStates.s1, undefined,
    'eviction must be refused when the stash cannot retain the draft');
  assert.equal(sessionStates.s1.composerDraft, 'x'.repeat(1000001),
    'the oversized unsent draft must survive in the resident buffer');
  assert.equal(sessionStates.s2, undefined,
    'the next eligible buffer (s2) is evicted instead');
  assert.equal(sessionStates.s3.composerDraft, 'd3', 'surviving buffers keep their small drafts');
  // The refusal must not wedge the LRU: another eligible session is evicted
  // instead, keeping the count bounded.
  assert.equal(Object.keys(sessionStates).length, 32,
    'the LRU must still evict an eligible buffer to stay bounded');
});

test('tauri stash capacity boundary: the 257th drafted session keeps its draft (eviction refused, no silent loss)', () => {
  const { api, state } = loadTauriSessionsFeature();
  // 32 resident + 257 evictions = 289 sessions: the 257th stash insert
  // would exceed the 256-entry side table.
  const total = 32 + 257;
  for (let i = 1; i <= total; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `draft-${i}`;
  }
  // s1 (oldest, draft already stashed earlier) was evicted; its stash is
  // still within the 256-entry table and restorable.
  assert.equal(api.getBuffer('s1').composerDraft, 'draft-1',
    'the earliest stashed draft is restored when the side table is within its entry bound');
  // The 257th eviction (s257's) was refused to avoid evicting the oldest
  // stash entry — the draft survives in the resident buffer.
  assert.equal(api.getBuffer('s257').composerDraft, 'draft-257',
    'the capacity-boundary draft is preserved (eviction refused rather than silently discarding input)');
});

// ── Review BLOCKER: fresh materialization draft-loss regression (tauri) ──

test('tauri fresh-materialized empty session with a draft: switching away and back via switchToSession keeps the draft', async () => {
  const { api, state, sessionStates } = loadTauriSessionsFeature({
    invoke(name, args) {
      // A session materialized by equipping a persona card is a zero-message
      // empty session on disk.
      if (name === 'load_session') {
        return Promise.resolve({
          metadata: { id: args.id, message_count: 0 },
          messages: [],
          artifacts: [],
        });
      }
      return Promise.resolve({});
    },
  });
  // Persona-card materialization: switchActiveTo(id, {fresh:true}) builds an
  // empty buffer (no messages).
  api.switchActiveTo('persona-1', { fresh: true });
  state.composerDraft = 'persona-draft'; // user's unsent draft
  // Switch away: saveWorkingSetTo snapshots the draft into persona-1's
  // background buffer.
  api.switchActiveTo('other', null);
  assert.equal(sessionStates['persona-1'].composerDraft, 'persona-draft', 'precondition: the draft was snapshotted into the buffer');
  // Switch back: the fresh empty buffer is marked loadedFromDisk → the fast
  // path hits and the draft survives. Before the regression fix the buffer
  // was unmarked, the gate rejected it onto the slow path, sessionStates[id]
  // was replaced by freshBuffer() without the side-table stash, and the
  // draft was silently dropped.
  assert.equal(await api.switchToSession('persona-1'), true);
  assert.equal(state.composerDraft, 'persona-draft',
    'the unsent draft of a fresh-materialized empty session must not be lost on switching back');
  assert.equal(sessionStates['persona-1'].loadedFromDisk, true,
    'a fresh empty buffer must be marked loadedFromDisk (the fast-path gate depends on it)');
});

// ── personaPlaceholderTitles cleaned only on real deletion ─────────────

test('tauri LRU capacity eviction keeps personaPlaceholderTitles; real deletion cleans it', () => {
  const personaPlaceholderTitles = {};
  const { api, state } = loadTauriSessionsFeature({ personaPlaceholderTitles });
  api.switchActiveTo('s1', null);
  personaPlaceholderTitles.s1 = true; // persona-card placeholder title marker
  state.composerDraft = 'd1';
  for (let i = 2; i <= 33; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `d${i}`;
  }
  assert.equal(personaPlaceholderTitles.s1, true,
    'capacity eviction must not delete the placeholder-title marker (rehydration never restores it)');

  api.purgeSessionBuffer('s1');
  assert.equal(personaPlaceholderTitles.s1, undefined,
    'real session deletion must clean the placeholder-title marker');
});

test('web personaPlaceholderTitles cleaned only on real deletion (source contract)', () => {
  const webBridge = read('bridge.js');
  const bodyOf = signature => {
    const start = webBridge.indexOf(signature);
    assert.ok(start > 0, `${signature} must exist`);
    const end = webBridge.indexOf('\n  function ', start + 1);
    return webBridge.slice(start, end > 0 ? end : undefined);
  };
  assert.ok(!/delete personaPlaceholderTitles/.test(bodyOf('function pruneSessionBuffers')),
    'the all-session LRU eviction must not clean personaPlaceholderTitles');
  assert.ok(!/delete personaPlaceholderTitles/.test(bodyOf('function pruneScheduledSessionBuffers')),
    'the scheduled LRU eviction must not clean personaPlaceholderTitles');
  assert.ok(/delete personaPlaceholderTitles\[id\]/.test(bodyOf('function purgeSessionBuffer')),
    'real session deletion (purgeSessionBuffer) must clean personaPlaceholderTitles');
});

// ── draft stash bounds: eviction is refused instead of losing input ────

test('tauri oversized draft (>1M chars) is never stashed-and-dropped: eviction is refused and the draft survives', () => {
  const { api, state, sessionStates } = loadTauriSessionsFeature();
  api.switchActiveTo('s1', null);
  // A transport-level write can bypass the composer input cap, producing an
  // oversized draft. It must not be silently lost at eviction time.
  state.composerDraft = 'y'.repeat(1000001);
  for (let i = 2; i <= 33; i++) {
    api.switchActiveTo(`s${i}`, null);
    state.composerDraft = `d${i}`;
  }
  // s1 is the oldest idle buffer but its draft exceeds the stash char
  // bound: the eviction is refused and the draft stays resident.
  assert.notEqual(sessionStates.s1, undefined,
    'a buffer whose draft cannot be safely retained must not be evicted');
  assert.equal(sessionStates.s1.composerDraft.length, 1000001,
    'the oversized draft survives in the resident buffer');
  // The LRU stays bounded by evicting the next eligible session instead.
  assert.equal(Object.keys(sessionStates).length, 32,
    'the buffer count stays bounded via other eligible evictions');
});

// ── P2: scene cache key semantics (source contract + tauri hook reason) ──

test('scene-events localStorage key cleaned only on real session deletion (tauri hook contract)', () => {
  const tauriBridge = fs.readFileSync(path.join(bridgeDir, '..', 'bridge.js'), 'utf8');
  // onSessionBufferPurged must distinguish by reason; only delete cleans
  // the scene cache key.
  assert.match(tauriBridge, /onSessionBufferPurged: function \(id, reason\)/,
    'the tauri bridge hook must receive the eviction reason');
  const sceneRemoves = tauriBridge.match(/removeItem\(PINVOU_SCENE_EVENTS_STORAGE_PREFIX \+ id\)/g) || [];
  assert.equal(sceneRemoves.length, 1, 'the tauri bridge must have exactly one scene-key cleanup (inside the hook)');
  const hookBody = tauriBridge.slice(
    tauriBridge.indexOf('onSessionBufferPurged: function (id, reason)'),
    tauriBridge.indexOf('onSessionBufferPurged: function (id, reason)') + 1600);
  const hookTail = hookBody.slice(hookBody.indexOf('reason === "delete"'));
  assert.match(hookTail, /removeItem\(PINVOU_SCENE_EVENTS_STORAGE_PREFIX/,
    'the scene-key cleanup must be gated on reason === "delete"');

  // Web bridge: neither LRU eviction loop may clean the scene key; cleanup
  // exists only in purgeSessionBuffer.
  const webBridge = read('bridge.js');
  const webSceneRemoves = webBridge.match(/removeItem\(PINVOU_SCENE_EVENTS_STORAGE_PREFIX \+ id\)/g) || [];
  assert.equal(webSceneRemoves.length, 1, 'the web bridge\'s scene-key cleanup must exist only in purgeSessionBuffer (real deletion)');
  const purgeAt = webBridge.indexOf('function purgeSessionBuffer');
  const pruneAt = webBridge.indexOf('function pruneSessionBuffers');
  assert.ok(purgeAt > 0 && pruneAt > 0);
  assert.ok(webSceneRemoves[0] && webBridge.indexOf(webSceneRemoves[0]) > purgeAt,
    'the web scene-key cleanup must live inside purgeSessionBuffer');
});

// ── web bridge: full-bridge vm boot driving the public API ────────────

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
  // Cold switches go through loadSessionForClient's chunk protocol: a
  // single-chunk eof response.
  handlers.web_access_load_session_chunk = args => {
    calls.chunkLoads.push(args.id);
    const saved = {
      metadata: { id: args.id, title: `session ${args.id}`, message_count: 1 },
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
    flat, storage, calls, handlers, deferreds, listeners,
    view() { return flat.getState(); },
  };
}

// ── P1 (web): drafts survive 33+ session eviction ─────────────────────

test('web 34-session LRU: evicted s1 revisited — draft restored via the side table with a disk rehydration', async () => {
  const rt = bootWebBridge();
  // s1 cold-loads (slow path builds a loadedFromDisk buffer) and leaves an
  // unsent draft.
  assert.equal(await rt.flat.switchToSession('s1'), true);
  rt.flat.setComposerDraft('web-unsent-draft');
  // Switch through 33 more sessions: the 34th session's creation evicts s1
  // (oldest idle).
  for (let i = 2; i <= 34; i++) {
    assert.equal(await rt.flat.switchToSession(`s${i}`), true);
  }
  const loadsBefore = rt.calls.chunkLoads.filter(id => id === 's1').length;
  assert.equal(loadsBefore, 1, 'precondition: s1 was cold-loaded exactly once before');
  // Switch back to s1: the buffer was evicted → the chunk rehydration must
  // run again (proving the eviction really happened).
  assert.equal(await rt.flat.switchToSession('s1'), true);
  const loadsAfter = rt.calls.chunkLoads.filter(id => id === 's1').length;
  assert.equal(loadsAfter, 2, 'a revisited evicted session must rehydrate (heavy objects really released)');
  assert.equal(rt.flat.getComposerDraft(), 'web-unsent-draft',
    'the unsent draft at eviction time must be restored via the side table');
});

test('web oversized draft (>1M chars) survives: eviction is refused instead of silently dropping input', async () => {
  const rt = bootWebBridge();
  assert.equal(await rt.flat.switchToSession('s1'), true);
  // A transport-level setter can bypass the composer input cap, producing
  // an oversized draft. It must not be lost to eviction.
  rt.flat.setComposerDraft('x'.repeat(1000001));
  for (let i = 2; i <= 34; i++) {
    assert.equal(await rt.flat.switchToSession(`s${i}`), true);
  }
  // s1 is the oldest idle session but its draft exceeds the stash char
  // bound: the eviction is refused and the draft survives in the resident
  // buffer (no rehydration needed, no silent loss).
  assert.equal(await rt.flat.switchToSession('s1'), true);
  assert.equal(rt.flat.getComposerDraft(), 'x'.repeat(1000001),
    'an oversized unsent draft must survive eviction (eviction refused rather than dropped)');
});

// ── P2 (web): failed save + eviction + rehydration ─────────────────────

test('web scene sidecar save failure + capacity eviction: the localStorage cache still recovers', async () => {
  const rt = bootWebBridge();
  const key = 'pinvou_scene_events_v1:s1';
  // Simulate "sidecar has no data yet and the backend save fails": get
  // returns an empty array, save rejects, and the localStorage cache is
  // the only copy (savePinvouSceneEventsForSession intentionally swallows
  // the backend failure, leaving the cache).
  rt.handlers.get_session_pinvou_scene_events = () => [];
  rt.handlers.save_session_pinvou_scene_events = () => Promise.reject(new Error('offline'));
  rt.storage.set(key, JSON.stringify([{ pos: 0, scene: 'work:document-writing' }]));

  assert.equal(await rt.flat.switchToSession('s1'), true);
  assert.equal(rt.view().pinvouSceneEvents.length, 1,
    'with an empty sidecar and a failed save, the localStorage cache must back the recovery');
  assert.equal(rt.storage.has(key), true);

  for (let i = 2; i <= 34; i++) {
    assert.equal(await rt.flat.switchToSession(`s${i}`), true);
  }
  // Capacity eviction must not clean the recovery copy.
  assert.equal(rt.storage.has(key), true,
    'LRU capacity eviction must not delete the only offline recovery copy of scene events');
  // Rehydration after eviction still recovers.
  assert.equal(await rt.flat.switchToSession('s1'), true);
  assert.equal(rt.view().pinvouSceneEvents.length, 1,
    'after eviction + rehydration the scene mapping must recover from the cache');

  // Control: real session deletion cleans the cache key.
  rt.handlers.delete_session = () => ({});
  assert.equal(await rt.flat.deleteSession('s1'), true);
  assert.equal(rt.storage.has(key), false,
    'real session deletion must clean the scene cache key (preventing unbounded accumulation)');
});

// ── scheduled-run reopen: draft recovery via the scheduled open flow after eviction ───

// Both openScheduledRunChatOnce branches (beginScheduledOpenActivation for
// queued/running, scheduledRunBuffer otherwise) rebuild the buffer via
// getBuffer first — the restore happens at the getBuffer restore point
// (the switchToSessionInternal hydrateLive entry restore is a defensive
// backstop, unreachable on the normal call graph). This test pins the
// reachable path: eviction → scheduled-run reopen → the draft must return
// to the visible working set.

test('web scheduled-run reopen: after eviction, the scheduled open restores the draft via the getBuffer rebuild', async () => {
  const rt = bootWebBridge();
  // The sched session was visited earlier and left an unsent draft, then
  // was evicted by capacity (buffer gone, draft only in the side table).
  // Reopening a running run takes the hydrateLiveSession: true path.
  const sid = 'sched-run-1';
  assert.equal(await rt.flat.switchToSession(sid), true);
  rt.flat.setComposerDraft('sched-live-draft');
  for (let i = 1; i <= 33; i++) {
    assert.equal(await rt.flat.switchToSession(`s${i}`), true);
  }
  assert.equal(await rt.flat.openScheduledRunChat(
    { sessionId: sid, status: 'running', runId: 'run-1' },
    { id: 'task-1' },
  ), true);
  assert.equal(rt.flat.getComposerDraft(), 'sched-live-draft',
    'a scheduled-run reopen after eviction (getBuffer rebuild) must restore the side-table draft');
  assert.equal(rt.view().pinvouSceneEvents.length, 0, 'hydration succeeded: the session is open');
  assert.notEqual(rt.view().activeSessionId, null);
});

// ── web getBuffer restore: the rebuild point for late events ──────────

test('web late chat:usage event: buffer rebuilt by the event via getBuffer, draft restored', async () => {
  const rt = bootWebBridge();
  assert.equal(await rt.flat.switchToSession('s1'), true);
  rt.flat.setComposerDraft('late-event-draft');
  for (let i = 2; i <= 34; i++) {
    assert.equal(await rt.flat.switchToSession(`s${i}`), true);
  }
  // s1 was evicted; a non-turn event (chat:usage) naming s1 →
  // onSessionEvent rebuilds an empty buffer via getBuffer → the stashed
  // draft is restored. Switching back then takes the fast path with the
  // draft intact.
  const listeners = rt.listeners && rt.listeners['chat:usage'];
  assert.ok(Array.isArray(listeners) && listeners.length > 0, 'the chat:usage listener is registered');
  for (const fn of listeners) {
    await fn({ event: 'chat:usage', payload: { session_id: 's1', input_tokens: 10 } });
  }
  assert.equal(await rt.flat.switchToSession('s1'), true);
  assert.equal(rt.flat.getComposerDraft(), 'late-event-draft',
    'a late event rebuilding the buffer via getBuffer must restore the side-table draft');
});
