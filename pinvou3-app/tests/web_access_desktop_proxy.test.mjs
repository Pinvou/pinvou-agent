import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = fs.readFileSync(
  path.join(root, 'src', 'platform', 'tauri', 'bridge', 'remote-control.js'),
  'utf8',
);

const listeners = new Map();
const listenCalls = [];
const invokeCalls = [];
const responses = [];
const publishedEvents = [];
let policyUrl = null;
let readyGeneration = null;
let notifications = 0;

const windowObject = {
  __PINVOU_TAURI_BRIDGE_FEATURES__: {},
  crypto: { randomUUID: () => '11111111-2222-3333-4444-555555555555' },
};
const context = vm.createContext({
  window: windowObject,
  document: { baseURI: 'https://desktop.local/' },
  URL,
  console,
  fetch: async url => {
    policyUrl = String(url);
    return {
      ok: true,
      json: async () => ({
        allowed_commands: ['list_sessions'],
        allowed_events: ['chat:delta'],
      }),
    };
  },
});
vm.runInContext(source, context, { filename: 'platform/tauri/bridge/remote-control.js' });

const state = { webAccess: { status: 'connecting' } };
async function listen(name, handler) {
  listenCalls.push(name);
  listeners.set(name, handler);
  return function () {};
}
async function invoke(command, args) {
  invokeCalls.push([command, args]);
  if (command === 'web_access_rpc_begin') return true;
  if (command === 'list_sessions') return [{ id: 'session-1', title: '测试对话' }];
  if (command === 'web_access_rpc_respond') {
    responses.push(args);
    return null;
  }
  if (command === 'web_access_publish_event') {
    publishedEvents.push(args);
    return null;
  }
  if (command === 'web_access_bridge_ready') {
    readyGeneration = args.generation;
    return null;
  }
  return null;
}

const factory = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['remote-control'];
assert.equal(typeof factory, 'function');
const feature = factory({
  state,
  listen,
  invoke,
  notify: () => { notifications += 1; },
});

await feature.startDesktopProxy();

assert.equal(policyUrl, 'https://desktop.local/platform/web/access-policy.json');
assert.equal(readyGeneration, 'webview_11111111_2222_3333_4444_555555555555');
for (const event of [
  'web_access:rpc_request',
  'web_access:event_subscribe',
  'web_access:event_unsubscribe',
  'web_access:status',
  'chat:delta',
]) {
  assert.equal(typeof listeners.get(event), 'function', `${event} listener must be installed`);
}

const callsBeforeRpc = invokeCalls.length;
await listeners.get('web_access:rpc_request')({
  payload: {
    request_id: 'request-1',
    bridge_generation: readyGeneration,
    command: 'list_sessions',
    args: { includeArchived: false },
  },
});
assert.deepEqual(
  invokeCalls.slice(callsBeforeRpc).map(([command]) => command),
  ['web_access_rpc_begin', 'list_sessions', 'web_access_rpc_respond'],
  'desktop proxy must execute the guarded RPC round trip in order',
);
assert.deepEqual(JSON.parse(JSON.stringify(responses.at(-1))), {
  requestId: 'request-1',
  generation: readyGeneration,
  ok: true,
  result: [{ id: 'session-1', title: '测试对话' }],
  error: null,
});

await listeners.get('web_access:status')({
  payload: { status: 'connected', connected: true },
});
assert.deepEqual(JSON.parse(JSON.stringify(state.webAccess)), {
  status: 'connected',
  connected: true,
});
assert.equal(notifications, 1, 'live desktop relay status must notify the UI');

await listeners.get('chat:delta')({ payload: { text: '你好' } });
assert.deepEqual(JSON.parse(JSON.stringify(publishedEvents.at(-1))), {
  event: 'chat:delta',
  payload: { text: '你好' },
});

const listenCount = listenCalls.length;
const readyCount = invokeCalls.filter(([command]) => command === 'web_access_bridge_ready').length;
await feature.startDesktopProxy();
assert.equal(listenCalls.length, listenCount, 'desktop proxy registration must be idempotent');
assert.equal(
  invokeCalls.filter(([command]) => command === 'web_access_bridge_ready').length,
  readyCount,
  'duplicate startup must not send a second readiness ACK',
);

console.log('web access desktop RPC round-trip test passed');

const sessionsSource = fs.readFileSync(
  path.join(root, 'src', 'platform', 'tauri', 'bridge', 'sessions.js'),
  'utf8',
);
const sessionListeners = new Map();
const deletedSessionId = 'deleted-from-web';
let sessionNotifications = 0;
let backendSessions = [{ id: deletedSessionId }, { id: 'kept-session' }];
let backendArchivedSessions = [{ id: deletedSessionId }];
const loadedSessionModels = [];
let personaSyncs = 0;
const sessionWindow = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
const sessionContext = vm.createContext({ window: sessionWindow, console });
vm.runInContext(sessionsSource, sessionContext, {
  filename: 'platform/tauri/bridge/sessions.js',
});

const sessionState = {
  activeSessionId: deletedSessionId,
  sessions: [{ id: deletedSessionId }, { id: 'kept-session' }],
  archivedSessions: [{ id: deletedSessionId }],
  scheduledTaskRecentRuns: [{ sessionId: deletedSessionId }],
  scheduledTaskRuns: [{ sessionId: deletedSessionId }],
  scheduledRunContext: { sessionId: deletedSessionId },
  scheduledTaskCreationSessionId: deletedSessionId,
  messages: [{ role: 'user', content: 'stale' }],
  chatItems: [{ type: 'user', text: 'stale' }],
  artifacts: [{ path: 'stale.txt' }],
  personaEvents: [],
  pinvouReviews: [],
  busy: false,
  planSnapshot: { plan: null, todos: null },
  modeState: { mode: 'yolo' },
  thinking: { active: false },
  tokens: { input: 0, max: 1000 },
  queued: [],
  activePersona: null,
  mountedCollection: null,
  scheduledTaskDraft: null,
};
const sessionStates = { [deletedSessionId]: { messages: sessionState.messages } };
const sessionFactory = sessionWindow.__PINVOU_TAURI_BRIDGE_FEATURES__.sessions;
sessionFactory({
  state: sessionState,
  invoke: async command => {
    if (command === 'list_sessions') return backendSessions;
    if (command === 'list_archived_sessions') return backendArchivedSessions;
    return null;
  },
  listen: async (name, handler) => {
    sessionListeners.set(name, handler);
    return function () {};
  },
  notify: () => { sessionNotifications += 1; },
  sessionStates,
  scheduledRunSessionOwners: { [deletedSessionId]: {} },
  personaPlaceholderTitles: { [deletedSessionId]: 'stale' },
  turnUsageDirty: { [deletedSessionId]: true },
  loadSessionModel: async id => { loadedSessionModels.push(id); },
  syncActivePersona: async () => { personaSyncs += 1; },
  invalidateScheduledRecentRunsForSession() {},
  currentStreamText: 'stale',
  currentStreamId: 1,
  pendingAssistantText: 'stale',
  pendingAssistantBlocks: [{}],
  itemIdSeq: 1,
  toolMeta: {},
});

assert.equal(typeof sessionListeners.get('session:deleted'), 'function');
sessionListeners.get('session:deleted')({ payload: { id: deletedSessionId } });
assert.deepEqual(
  JSON.parse(JSON.stringify(sessionState.sessions)),
  [{ id: 'kept-session' }],
);
assert.equal(sessionState.activeSessionId, null);
assert.deepEqual(JSON.parse(JSON.stringify(sessionState.messages)), []);
assert.equal(sessionStates[deletedSessionId], undefined);
assert.equal(sessionNotifications, 1);

console.log('web access desktop session deletion sync test passed');

backendSessions = [
  { id: 'created-from-web', title: 'new' },
  { id: 'kept-session', title: 'renamed', pinned: true },
];
backendArchivedSessions = [{ id: 'archived-from-web' }];
assert.equal(typeof sessionListeners.get('session:list_changed'), 'function');
sessionListeners.get('session:list_changed')({
  payload: { id: 'kept-session', action: 'renamed' },
});
await new Promise(resolve => setTimeout(resolve, 0));
assert.deepEqual(
  JSON.parse(JSON.stringify(sessionState.sessions)),
  backendSessions,
  'desktop session list must refresh after WebUI create/rename/pin changes',
);
assert.deepEqual(
  JSON.parse(JSON.stringify(sessionState.archivedSessions)),
  backendArchivedSessions,
  'desktop archive list must refresh after WebUI archive/restore changes',
);
assert.equal(sessionNotifications, 2);

console.log('web access desktop session list sync test passed');

sessionState.activeSessionId = 'kept-session';
assert.equal(typeof sessionListeners.get('session:model_changed'), 'function');
sessionListeners.get('session:model_changed')({
  payload: { id: 'kept-session', action: 'model' },
});
assert.equal(typeof sessionListeners.get('session:persona_changed'), 'function');
sessionListeners.get('session:persona_changed')({
  payload: { id: 'kept-session', action: 'equipped' },
});
await new Promise(resolve => setTimeout(resolve, 0));
assert.deepEqual(loadedSessionModels, ['kept-session']);
assert.equal(personaSyncs, 1);
assert.equal(sessionNotifications, 3);

sessionListeners.get('session:model_changed')({ payload: { id: 'other-session' } });
sessionListeners.get('session:persona_changed')({ payload: { id: 'other-session' } });
await new Promise(resolve => setTimeout(resolve, 0));
assert.deepEqual(loadedSessionModels, ['kept-session']);
assert.equal(personaSyncs, 1);

console.log('web access desktop active session state sync test passed');
