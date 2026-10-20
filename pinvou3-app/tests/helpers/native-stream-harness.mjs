import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { marked } from 'marked';

const app = new URL('../../', import.meta.url);
const read = relative => readFileSync(new URL(relative, app), 'utf8');
const sharedSource = read('src/shared/bridge-shared-helpers.js');
const eventSource = read('src/platform/tauri/bridge/chat-events.js');
const hostSource = read('src/platform/tauri/bridge.js');

export const nativeStreamSources = { events: eventSource, host: hostSource };

function notificationSource(source) {
  const start = source.indexOf('  function notify() {');
  const end = source.indexOf('  function subscribe(', start);
  if (start < 0 || end < 0) throw new Error('Native host notification boundary not found');
  return source.slice(start, end);
}

/** Real handler/router/host-notify execution; transport, storage and buffers are isolated. */
export function createNativeStreamHarness({ count = 2, events = eventSource, host = hostSource,
  frames = true, timers = true, render = text => marked.parse(text), recordSnapshots = true,
  onNotify = () => {} } = {}) {
  let now = 0;
  let timerId = 0;
  let suppressed = 0;
  let notificationCount = 0;
  const jobs = new Map();
  const listeners = new Map();
  const sessions = {};
  const snapshots = [];
  const renders = [];
  const errors = [];
  const storageCalls = [];
  const later = (callback, delay = 0) => {
    const id = ++timerId;
    jobs.set(id, { callback, at: now + delay });
    return id;
  };
  const window = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  if (frames) {
    window.requestAnimationFrame = callback => later(callback, 16);
    window.cancelAnimationFrame = id => jobs.delete(id);
  }
  const state = { activeSessionId: 's0', sessions: Array.from({ length: count }, (_, i) => ({ id: `s${i}` })) };
  const context = {
    state, sessionStates: sessions, turnUsageDirty: {},
    listen: (name, callback) => listeners.set(name, callback),
    invoke: async () => null, bt: key => key, timeStr: () => '12:00',
    isNotifySuppressed: () => suppressed > 0,
    renderMarkdown(text) {
      renders.push({ sessionId: state.activeSessionId, at: now, length: text.length });
      return render(text);
    },
    addChatItem(item) { item.id = ++context.itemIdSeq; state.chatItems.push(item); },
    addSystemItem(text, meta) { context.addChatItem({ type: 'system', text, ...meta }); },
    toolCallAlreadyStarted: () => false, toolCallAlreadyFinished: () => false,
    hasChatItemForTool: () => false, isPresentArtifactTool: () => false,
    startThinking() {}, stopThinking() {}, thinkingTool() {},
    flushPendingTextBlock() {}, flushQueued() {},
    safeConsoleInfo() {}, isScheduledRunSession: () => false,
    flushAssistantMessageToHistory() {
      state.messages.push({ role: 'assistant', content: context.currentStreamText });
    },
    persistMessagesFor: async sid => { storageCalls.push(sid); },
    refreshHistoryList: async () => {}, reconcileRemoteTurn: async () => true,
    hydratedMessageKey: () => '', markRemoteTurn() {},
    saveWorkingSetTo(buffer) { Object.assign(buffer, stateFields()); },
  };
  const stateKeys = ['chatItems', 'messages', 'thinking', 'busy', 'tokens', 'localTurnOwned',
    'turnTimeline', 'activeTurnTimelineId', 'artifacts', 'turnDirtyArtifacts', 'turnPresentedArtifacts'];
  const contextKeys = ['currentStreamId', 'currentStreamText', 'pendingAssistantText',
    'pendingAssistantBlocks', 'itemIdSeq', 'toolMeta'];
  const streamStates = {};
  function stateFields() { return Object.fromEntries(stateKeys.map(key => [key, state[key]])); }
  function save(sid) {
    Object.assign(sessions[sid], stateFields());
    streamStates[sid] = Object.fromEntries(contextKeys.map(key => [key, context[key]]));
  }
  function load(sid) {
    for (const key of stateKeys) state[key] = sessions[sid][key];
    Object.assign(context, streamStates[sid]);
  }
  for (let i = 0; i < count; i++) {
    const sid = `s${i}`;
    sessions[sid] = { chatItems: [], messages: [], thinking: { active: true }, busy: true,
      tokens: { input: 0, max: 0 }, localTurnOwned: true, artifacts: [],
      turnDirtyArtifacts: [], turnPresentedArtifacts: [], activeTurnTimelineId: `turn-${sid}`,
      turnTimeline: [{ event: 'user_start', turn_id: `turn-${sid}`, timestamp: 1, ui_turn_index: 0 }] };
    streamStates[sid] = { currentStreamId: 0, currentStreamText: '', pendingAssistantText: '',
      pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {} };
  }
  load('s0');
  function route(sid, callback) {
    if (sid === state.activeSessionId) { callback(); return; }
    if (!sessions[sid]) return;
    const active = state.activeSessionId;
    save(active);
    load(sid);
    state.activeSessionId = sid;
    suppressed++;
    try { callback(); }
    finally { save(sid); suppressed--; state.activeSessionId = active; load(active); }
  }
  context.runSyncOnSession = route;
  context.getBuffer = sid => { if (sid === state.activeSessionId) save(sid); return sessions[sid]; };
  const sandbox = vm.createContext({ window, Date, String, state, sessionStates: sessions,
    startupNotificationBatching: false, refreshActiveStreamRender: null,
    notificationQueue: [], notificationDispatching: false, subscriptionSliceRevision: 0,
    console: { ...console, error: (...args) => errors.push(args) },
    subscribers: [{ snapshot: () => {
      const assistant = state.chatItems.find(item => item.type === 'assistant');
      return { sessionId: state.activeSessionId, text: assistant?.text, html: assistant?.html, busy: state.busy };
    }, callback: snapshot => {
      notificationCount++;
      if (recordSnapshots) snapshots.push(snapshot);
      onNotify(snapshot);
    } }],
    ...(timers ? { setTimeout: later, clearTimeout: id => jobs.delete(id) } : {}),
  });
  Object.defineProperty(sandbox, 'suppressNotify', { get: () => suppressed > 0 });
  vm.runInContext(sharedSource + '\n' + notificationSource(host) + '\n' + events, sandbox,
    { filename: fileURLToPath(new URL('src/platform/tauri/bridge/chat-events.js', app)) });
  context.notify = sandbox.notify;
  context.onSessionEvent = window.PinvouBridgeShared.create('tauriMain', {
    state, sessionStates: sessions, getBuffer: context.getBuffer,
    runSyncOnSession: route, notify: context.notify,
  }).onSessionEvent;
  const feature = window.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'](context);
  sandbox.refreshActiveStreamRender = feature.refreshActiveStreamRender;
  return {
    state, context, sessions, renders, snapshots, errors, storageCalls, feature,
    get notificationCount() { return notificationCount; },
    get now() { return now; },
    emit(sid, name, payload = {}) {
      listeners.get(name)({ event: name, payload: { session_id: sid, ...payload } });
    },
    advance(target) {
      if (target < now) throw new Error('Virtual clock cannot move backwards');
      for (;;) {
        let next;
        for (const entry of jobs) if (entry[1].at <= target && (!next || entry[1].at < next[1].at)) next = entry;
        if (!next) break;
        now = next[1].at; jobs.delete(next[0]); next[1].callback();
      }
      now = target;
    },
    activate(sid) {
      save(state.activeSessionId);
      state.activeSessionId = sid;
      load(sid);
      context.notify();
    },
    view(sid) { if (sid === state.activeSessionId) save(sid); return sessions[sid]; },
    purge(sid) { feature.cancelStreamRenderTimers(sid); delete sessions[sid]; delete streamStates[sid]; },
    notify() { context.notify(); },
  };
}
