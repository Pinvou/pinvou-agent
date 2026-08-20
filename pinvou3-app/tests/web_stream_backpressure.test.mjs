#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridgePath = path.join(root, 'src', 'platform', 'web', 'bridge.js');
const domainAdapterPath = path.join(root, 'src', 'platform', 'web', 'bridge', 'domain-adapter.js');
const productionSource = readFileSync(bridgePath, 'utf8');
const domainAdapterSource = readFileSync(domainAdapterPath, 'utf8');
const exposeMarker = '  // ── Expose API ─────────────────────────────────';
assert.ok(productionSource.includes(exposeMarker), 'Web bridge test seam marker moved');

function fakeTimers() {
  let sequence = 0;
  const active = new Map();
  const delays = [];
  return {
    setTimeout(callback, delay) {
      const id = ++sequence;
      active.set(id, callback);
      delays.push(Number(delay));
      return id;
    },
    clearTimeout(id) {
      active.delete(id);
    },
    fireNext() {
      const next = active.entries().next();
      if (next.done) return false;
      const [id, callback] = next.value;
      active.delete(id);
      callback();
      return true;
    },
    count() { return active.size; },
    delays,
  };
}

function createHarness() {
  const timers = fakeTimers();
  const listeners = new Map();
  const storage = new Map();
  let markdownCalls = 0;
  let deepCloneCalls = 0;
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
  const windowObject = {
    PinvouPlatform: {
      kind: 'web',
      isWeb: true,
      capabilities: {},
      can: () => false,
      canInvoke: () => false,
    },
    PinvouMarkdownRenderer: {
      renderMarkdown(text) {
        markdownCalls += 1;
        return `<md>${text}</md>`;
      },
    },
    PinvouBridgeMessages: { showShellCleanupFailure() {} },
    PinvouWebTurnTerminal: { recordCompleted() {} },
    PinvouAttachmentDropController: { install() {} },
    __TAURI__: {
      core: {
        invoke: async () => null,
        invokeWithRequestId: async () => null,
      },
      event: {
        listen(name, handler) {
          listeners.set(name, handler);
          return Promise.resolve(() => {});
        },
      },
      dialog: { open: async () => null },
    },
    location: { search: '', href: 'https://example.test/pinvou3/remote/' },
    localStorage,
    crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
    performance: { now: () => 0 },
    addEventListener() {},
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
  };

  const injectedSource = productionSource.replace(exposeMarker, `
  window.__WEB_STREAM_TEST__ = {
    state: state,
    sessionStates: sessionStates,
    resetActive: function (sid) {
      Object.keys(pendingChatDeltas).forEach(discardPendingChatDeltas);
      Object.keys(terminalChatStreams).forEach(function (id) { delete terminalChatStreams[id]; });
      Object.keys(sessionStates).forEach(function (id) { delete sessionStates[id]; });
      var buffer = freshBuffer();
      buffer.loadedFromDisk = true;
      buffer.localTurnOwned = true;
      buffer.busy = true;
      sessionStates[sid] = buffer;
      state.sessions = [{ id: sid, title: 'Streaming' }];
      state.activeSessionId = sid;
      loadWorkingSetFrom(buffer);
      state.busy = true;
      state.thinking = { active: true, phase: 'thinking', toolName: '', startedAt: 1 };
      saveWorkingSetTo(buffer);
      return buffer;
    },
    createBackground: function (sid) {
      var buffer = freshBuffer();
      buffer.loadedFromDisk = true;
      buffer.busy = true;
      sessionStates[sid] = buffer;
      return buffer;
    },
    pendingBlocks: function () { return pendingAssistantBlocks; },
    pendingLength: function (sid) {
      return pendingChatDeltas[sid] ? pendingChatDeltas[sid].pendingLength : 0;
    },
    flush: flushPendingChatDeltas,
    discard: discardPendingChatDeltas,
    purge: purgeSessionBuffer,
    markTerminal: function (sid) {
      terminalChatStreams[sid] = true;
      var buffer = sessionStates[sid];
      if (buffer) buffer.busy = false;
      if (state.activeSessionId === sid) state.busy = false;
    },
  };

${exposeMarker}`);

  const clone = value => {
    deepCloneCalls += 1;
    return structuredClone(value);
  };
  const context = vm.createContext({
    window: windowObject,
    document: documentObject,
    navigator: { mediaDevices: null },
    localStorage,
    console,
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
    setInterval: () => 1,
    clearInterval() {},
    structuredClone: clone,
    URL,
    URLSearchParams,
    Blob,
    Uint8Array,
    ArrayBuffer,
    TextEncoder,
    TextDecoder,
  });
  vm.runInContext(injectedSource, context, { filename: 'platform/web/bridge.js' });
  const api = windowObject.__WEB_STREAM_TEST__;
  api.resetActive('session-1');
  return {
    api,
    timers,
    bridge: windowObject.TauriBridge,
    context,
    window: windowObject,
    emit(name, payload = {}) {
      const handler = listeners.get(name);
      assert.ok(handler, `missing listener ${name}`);
      return handler({ event: name, payload: { session_id: 'session-1', ...payload } });
    },
    counts() { return { markdownCalls, deepCloneCalls }; },
    resetCounts() { markdownCalls = 0; deepCloneCalls = 0; },
  };
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: 'A' });
  harness.emit('chat:delta', { text: 'B' });
  harness.emit('chat:delta', { text: 'C' });

  assert.equal(harness.timers.count(), 1, 'one Web Session must own at most one cadence timer');
  assert.equal(harness.counts().markdownCalls, 0, 'transport deltas must not parse Markdown');
  assert.equal(harness.counts().deepCloneCalls, 0, 'transport deltas must not clone application state');
  assert.equal(harness.api.state.chatItems[0].text, 'A', 'the first chunk establishes exact raw text synchronously');

  harness.emit('chat:tool_start', { id: 'tool-1', name: 'search', args: {} });
  assert.equal(harness.timers.count(), 0, 'a tool boundary must cancel the cadence timer');
  assert.equal(harness.api.state.chatItems[0].text, 'ABC');
  assert.equal(harness.api.state.chatItems[0].html, '<md>ABC</md>');
  assert.equal(harness.api.state.chatItems[0].streaming, false);
  assert.equal(harness.api.state.chatItems[0].streamingPreviewText, undefined);
  assert.deepEqual(
    JSON.parse(JSON.stringify(harness.api.pendingBlocks())),
    [
      { type: 'text', text: 'ABC' },
      { type: 'tool_use', id: 'tool-1', name: 'search', input: {} },
    ],
    'the exact text block must precede the following tool block',
  );
  assert.deepEqual(
    JSON.parse(JSON.stringify(harness.api.state.chatItems.map(item => item.type))),
    ['assistant', 'tool'],
  );
  assert.equal(harness.counts().markdownCalls, 1, 'the semantic boundary renders Markdown exactly once');
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: 'A' });
  harness.emit('chat:delta', { text: 'x'.repeat(9000) });
  assert.equal(harness.api.pendingLength('session-1'), 0, '8 KiB pending waterline must flush synchronously');
  assert.equal(harness.timers.count(), 0, 'waterline flush must clear the old cadence timer');
  assert.equal(harness.api.state.chatItems[0].text.length, 9001);
  assert.equal(harness.counts().deepCloneCalls, 0, 'waterline publication must stay shallow');
}

{
  const harness = createHarness();
  const deltaCount = 63_927;
  const totalChars = 135_770;
  const longChunkCount = totalChars - (deltaCount * 2);
  const expectedChunks = Array.from({ length: deltaCount }, (_, index) => (
    index < longChunkCount ? 'xxx' : 'xx'
  ));
  const expectedText = expectedChunks.join('');
  let maxPreviewChars = 0;

  expectedChunks.forEach((chunk, index) => {
    harness.emit('chat:delta', { text: chunk });
    if ((index + 1) % 100 === 0) {
      assert.equal(harness.timers.fireNext(), true);
      maxPreviewChars = Math.max(
        maxPreviewChars,
        harness.api.state.chatItems[0].streamingPreviewText.length,
      );
    }
  });
  harness.api.flush('session-1', { finalize: true, notify: true });

  assert.equal(harness.api.state.chatItems[0].text, expectedText, 'coalescing must preserve all 135,770 characters');
  assert.equal(maxPreviewChars, 16_384, 'live React text must have a hard 16 KiB character cap');
  assert.equal(harness.api.state.chatItems[0].streamingPreviewText, undefined);
  assert.equal(harness.api.state.chatItems[0].html, `<md>${expectedText}</md>`);
  assert.equal(harness.counts().markdownCalls, 1, '63,927 deltas still render Markdown only at terminal');
  assert.equal(harness.counts().deepCloneCalls, 0, 'all streaming publications stay off structuredClone');
  assert.equal(harness.timers.count(), 0);
  assert.ok(harness.timers.delays.includes(40) && harness.timers.delays.includes(160),
    'cadence must adapt from 40 ms to 160 ms for long answers');
}

{
  const harness = createHarness();
  const activeItems = harness.api.state.chatItems;
  const background = harness.api.createBackground('session-2');
  harness.emit('chat:delta', { session_id: 'session-2', text: 'background' });
  assert.equal(harness.api.state.chatItems, activeItems, 'background delta must restore the active working set');
  assert.equal(harness.api.state.chatItems.length, 0, 'background text must not leak into the active Session');
  assert.equal(background.chatItems[0].text, 'background');

  harness.emit('chat:delta', { session_id: 'session-2', text: '-pending' });
  assert.equal(harness.timers.count(), 1);
  harness.api.purge('session-2');
  assert.equal(harness.timers.count(), 0, 'purge must synchronously cancel the background timer');
  assert.equal(harness.timers.fireNext(), false, 'a purged timer cannot resurrect its Session');
  assert.equal(harness.api.sessionStates['session-2'], undefined);
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: 'Preparing\n```pers' });
  harness.emit('chat:delta', { text: 'ona-card\n{"name":"Expert","body":"secret"}' });
  assert.equal(harness.timers.fireNext(), true);
  assert.equal(
    harness.api.state.chatItems[0].streamingStructuredDraft,
    'persona-card',
    'an explicit structured marker split across transport deltas must be detected',
  );
  harness.emit('chat:tool_start', { id: 'structured-boundary', name: 'save', args: {} });
  assert.equal(
    harness.api.state.chatItems[0].streamingStructuredDraft,
    undefined,
    'semantic finalization must hand presentation back to the terminal Markdown/card parser',
  );
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: '```json\n{"na' });
  harness.emit('chat:delta', { text: 'me":"Fallback","body":"bounded"}' });
  assert.equal(harness.timers.fireNext(), true);
  assert.equal(
    harness.api.state.chatItems[0].streamingStructuredDraft,
    'structured',
    'a generic fenced card key split across deltas must use the bounded fallback',
  );
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: '```scheduled-' });
  harness.emit('chat:delta', {
    text: 'task-draft\n{"rrule":"daily","payload":"' + 'x'.repeat(22 * 1024) + '"}',
  });
  assert.equal(
    harness.api.state.chatItems[0].streamingStructuredDraft,
    'scheduled-task-draft',
    'the scheduled draft classification must survive a payload larger than the live preview',
  );
  assert.equal(harness.api.state.chatItems[0].streamingPreviewText.length, 16_384);
  assert.equal(
    harness.api.state.chatItems[0].streamingPreviewText.includes('scheduled-task-draft'),
    false,
    'the marker may slide out of the bounded preview without clearing sticky classification',
  );
  harness.emit('chat:delta', { text: 'y'.repeat(9000) });
  assert.equal(harness.api.state.chatItems[0].streamingStructuredDraft, 'scheduled-task-draft');
  assert.equal(harness.counts().markdownCalls, 0);
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: 'terminal' });
  harness.api.flush('session-1', { finalize: true });
  harness.api.markTerminal('session-1');
  harness.emit('chat:delta', { text: '-late' });
  assert.equal(harness.api.state.chatItems[0].text, 'terminal', 'late post-terminal deltas must be ignored while idle');
}

{
  const harness = createHarness();
  const flat = harness.window.TauriBridge;
  const originalGetState = flat.getState;
  let adapterGetStateCalls = 0;
  flat.getState = function () {
    adapterGetStateCalls += 1;
    return originalGetState();
  };
  vm.runInContext(domainAdapterSource, harness.context, {
    filename: 'platform/web/bridge/domain-adapter.js',
  });
  let latestChatState = null;
  harness.window.TauriBridge.state.subscribeMany(['chat'], function (snapshot) {
    latestChatState = snapshot;
  });
  adapterGetStateCalls = 0;
  harness.resetCounts();

  harness.emit('chat:delta', { text: 'hot' });
  assert.equal(harness.timers.fireNext(), true);
  assert.equal(adapterGetStateCalls, 0, 'a streaming publication must not re-enter flat.getState()');
  assert.equal(harness.counts().deepCloneCalls, 0, 'a streaming publication must not clone again in the adapter');
  assert.equal(latestChatState.chatItems[0].streamingPreviewText, 'hot');

  harness.emit('chat:tool_start', { id: 'tool-semantic', name: 'search', args: {} });
  assert.ok(adapterGetStateCalls > 0, 'ordinary semantic publications must retain isolated state reads');
  assert.ok(harness.counts().deepCloneCalls > 0, 'ordinary semantic publications must retain deep isolation');
  const internalLength = harness.api.state.chatItems.length;
  latestChatState.chatItems.push({ type: 'test-mutation' });
  assert.equal(
    harness.api.state.chatItems.length,
    internalLength,
    'mutating an ordinary adapter snapshot must never mutate bridge state',
  );
}

assert.match(
  productionSource,
  /function notifyStreamingChat\(\)[\s\S]*?snapshotStateShallow\(\)/,
  'Web hot notifications must use the shallow snapshot path',
);
assert.match(
  productionSource,
  /STREAMING_STRUCTURED_PROBE_CHAR_LIMIT = 1024[\s\S]*?new WeakMap\(\)[\s\S]*?streamingStructuredDraft/,
  'structured streaming detection must use bounded rolling probe state rather than the full answer',
);
assert.match(
  domainAdapterSource,
  /isStreamingPublication\(snapshot, publication\)[\s\S]*?pickMany\(snapshot, domains\)[\s\S]*?: getMany\(domains\)/,
  'the Web domain adapter must bypass its second deep read only for explicit streaming publications',
);

const chatViewSource = readFileSync(path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const voiceShellSource = readFileSync(path.join(root, 'src', 'features', 'pinvou_os', 'PinvouOsVoiceShell.jsx'), 'utf8');
assert.match(
  chatViewSource,
  /streamingStructuredDraft[\s\S]*?streamingDraftLabel[\s\S]*?\{streamingStructuredDraft \? \(/,
  'ChatView must render the existing design/draft placeholder before any raw preview',
);
assert.match(
  voiceShellSource,
  /visibleAssistant\.streamingStructuredDraft[\s\S]*?draftingScheduled[\s\S]*?cpDesigning/,
  'VoiceShell must render the same structured-draft placeholder instead of raw JSON',
);
assert.match(
  productionSource,
  /snapshot\.chatItems = state\.chatItems\.slice\(\)/,
  'Web hot snapshots must change chatItems array identity for React consumers',
);
assert.doesNotMatch(
  productionSource,
  /listen\("chat:delta"[\s\S]{0,600}renderMarkdown\(currentStreamText\)/,
  'the chat:delta listener must never render accumulated Markdown',
);
assert.match(
  productionSource,
  /function handleChatDone\(e\)[\s\S]{0,320}flushPendingChatDeltas\(sid, \{ finalize: true \}\)[\s\S]{0,2600}terminalChatStreams\[sid\] = true/,
  'chat:done must settle the stream and install a late-delta tombstone',
);

console.log('web_stream_backpressure: ok');
