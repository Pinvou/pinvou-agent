import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

// r52 P2 regression (review): the desktop listener's pre-capture results ride
// a hooks argument into the shared compaction handler. The wrapper used to
// accept only (e) and forward only (e), so the hooks were dropped,
// doneNotice never executed, and the foreground force-durable-reload was
// skipped - while every source-string pin stayed green. This test EXECUTES
// the real listener -> wrapper -> shared-handler forwarding chain in a vm
// sandbox with a recording shared factory.

const sandbox = { window: {}, console };
vm.runInNewContext(read('src', 'shared', 'bridge-shared-helpers.js'), sandbox, {
  filename: 'shared/bridge-shared-helpers.js',
});
vm.runInNewContext(read('src', 'platform', 'tauri', 'bridge', 'chat-events.js'), sandbox, {
  filename: 'chat-events.js',
});

const received = [];
// The shared module freezes its export object; replace the whole reference
// (the factory resolves window.PinvouBridgeShared.create at call time).
sandbox.window.PinvouBridgeShared = {
  create(name) {
    assert.equal(name, 'tauriChatEvents', 'the lane must address the tauriChatEvents shared instance');
    return {
      handleChatCompactionEvent(e, hooks) {
        received.push(hooks);
        if (hooks && typeof hooks.doneNotice === 'function') hooks.doneNotice();
        else notices.push({ text: '[done] [auto] ' + ((e.payload && e.payload.message) || '') });
      },
    };
  },
};

const listeners = {};
const notices = [];
let reloaded = 0;
let refuseInvalidation = false;

const state = {
  activeSessionId: 'sess-1',
  busy: false,
  queued: [],
  messages: [],
  chatItems: [],
  tokens: { input: 0, max: 1000 },
  compactingSessions: {},
  compactionStarts: {},
};

const context = {
  state,
  listen(name, fn) {
    listeners[name] = fn;
  },
  notify() {},
  isNotifySuppressed() { return false; },
  onSessionEvent(e, fn) { fn(); },
  runSyncOnSession(sid, fn) { fn(); },
  bt(key) {
    return {
      compactDone: '[done] ',
      compactAuto: '[auto] ',
      compactStart: '[start] ',
      compactFail: '[fail] ',
      compactCancel: '[cancel] ',
    }[key] || '';
  },
  addSystemItem(text, meta) { notices.push({ text, meta }); },
  addOrMergePruneCompaction() {},
  addChatItem() {},
  turnUsageDirty: {},
  timeStr() { return ''; },
  getBuffer() { return []; },
  invalidateSessionBuffer() { return refuseInvalidation ? false : true; },
  switchToSessionInternal() { reloaded += 1; return true; },
  itemIdSeq: 0,
  pendingStreamNotifications: [],
  pendingAssistantText: '',
  pendingAssistantBlocks: [],
  flushPendingTextBlock() {},
  autoCreateScheduledTaskDraft() {},
  finalizeAssistantStreamBeforeReasoning() {},
  applyAuthoritativeModeState() {},
  renderMarkdown(t) { return t; },
  invoke() {},
  sessionStates: {},
  toolCallAlreadyStarted() { return false; },
  toolCallAlreadyFinished() { return false; },
  hasChatItemForTool() { return false; },
  addAuthoritySyncNotice() {},
  flushAssistantMessageToHistory() {},
};

sandbox.window.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'](context);
const listener = listeners['chat:compaction'];
assert.ok(typeof listener === 'function', 'the chat:compaction listener must be registered');

const doneEvent = {
  payload: {
    session_id: 'sess-1',
    phase: 'done',
    messages_before: 10,
    messages_after: 4,
    auto: true,
    message: 'compacted',
  },
};

// 1) Foreground shrink with a successful invalidation: the hooks must REACH
//    the shared handler (the dropped-argument wrapper sent undefined) and the
//    foreground force-durable-reload must run inside doneNotice.
listener(doneEvent);
await new Promise((resolve) => {
      setImmediate(resolve);
    });
assert.equal(received.length, 1, 'the shared handler must receive exactly one hooks value');
assert.notEqual(received[0], undefined, 'hooks must not be dropped by the desktop wrapper');
assert.equal(typeof received[0].doneNotice, 'function', 'doneNotice must ride the hooks');
assert.equal(reloaded, 1, 'the foreground force-durable-reload must execute via doneNotice');
assert.ok(
  notices.some((n) => typeof n.text === 'string' && n.text.startsWith('[done] ')),
  'the done notice must mount after the successful reload',
);

// 2) refused invalidation (r46 MAJOR-3): invalidateSessionBuffer returns
//    false, so no hooks are injected (main semantics: plain notice) and the
//    reload is skipped.
refuseInvalidation = true;
listener(doneEvent);
await new Promise((resolve) => {
      setImmediate(resolve);
    });
assert.equal(received.length, 2);
assert.equal(received[1], undefined, 'a refused invalidation must not inject hooks');
assert.equal(reloaded, 1, 'a refused invalidation must skip the force-durable reload');
assert.ok(notices.length >= 2, 'the plain done notice still mounts on refusal');

// 3) background session (not the active one): the cache is invalidated (the
//    background leg) but the foreground reload arm does not run.
state.activeSessionId = 'sess-other';
refuseInvalidation = false;
listener(doneEvent);
await new Promise((resolve) => {
      setImmediate(resolve);
    });
assert.equal(received.length, 3);
assert.notEqual(received[2], undefined, 'a background shrink still invalidates and injects hooks');
assert.equal(reloaded, 1, 'the background arm must not force-reload the foreground view');

console.log('chat compaction hooks forwarding: ok');
