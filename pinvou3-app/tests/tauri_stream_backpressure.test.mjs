#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const chatEventsSource = readFileSync(
  path.join(root, 'src', 'platform', 'tauri', 'bridge', 'chat-events.js'),
  'utf8',
);
const chatSource = readFileSync(
  path.join(root, 'src', 'platform', 'tauri', 'bridge', 'chat.js'),
  'utf8',
);
const sessionsSource = readFileSync(
  path.join(root, 'src', 'platform', 'tauri', 'bridge', 'sessions.js'),
  'utf8',
);
const bridgeSource = readFileSync(
  path.join(root, 'src', 'platform', 'tauri', 'bridge.js'),
  'utf8',
);
const chatViewSource = readFileSync(
  path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'),
  'utf8',
);
const voiceShellSource = readFileSync(
  path.join(root, 'src', 'features', 'pinvou_os', 'PinvouOsVoiceShell.jsx'),
  'utf8',
);

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
    count() {
      return active.size;
    },
    delays,
  };
}

function createHarness() {
  const timers = fakeTimers();
  const listeners = new Map();
  const windowObject = {
    __PINVOU_TAURI_BRIDGE_FEATURES__: {},
    PinvouBridgeMessages: { showShellCleanupFailure() {} },
  };
  vm.runInContext(chatEventsSource, vm.createContext({
    window: windowObject,
    console,
    Date,
    String,
  }), { filename: 'chat-events.js' });

  const state = {
    activeSessionId: 'session-1',
    sessions: [{ id: 'session-1', title: 'Streaming' }],
    chatItems: [{ id: 1, type: 'assistant', html: '', streaming: true }],
    messages: [],
    thinking: { active: true, phase: 'thinking' },
    busy: true,
    queued: [],
    turnTimeline: [],
    artifacts: [],
    turnDirtyArtifacts: [],
    turnPresentedArtifacts: [],
    modeState: { mode: 'yolo' },
  };
  const sessionStates = {
    'session-1': {
      busy: true,
      localTurnOwned: true,
      messages: state.messages,
      chatItems: state.chatItems,
      queued: state.queued,
    },
  };
  let markdownCalls = 0;
  let deepNotifyCalls = 0;
  let streamingNotifyCalls = 0;
  const context = {
    state,
    sessionStates,
    listen(name, handler) { listeners.set(name, handler); },
    invoke: async command => command === 'get_session_timeline' ? [] : null,
    notify() { deepNotifyCalls += 1; },
    notifyStreamingChat() { streamingNotifyCalls += 1; },
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
    turnUsageDirty: {},
    renderMarkdown(text) {
      markdownCalls += 1;
      return `<md>${text}</md>`;
    },
    bt() { return ''; },
    onSessionEvent(_event, callback) { callback(); },
    runSyncOnSession(_sessionId, callback) { callback(); },
    applyAuthoritativeModeState() {},
    addChatItem(item) {
      item.id = item.id || ++context.itemIdSeq;
      state.chatItems.push(item);
      return item;
    },
    addSystemItem(text, meta = {}) {
      return context.addChatItem({ type: 'system', text, ...meta });
    },
    addAuthoritySyncNotice() {},
    timeStr() { return '12:00'; },
    toolCallAlreadyStarted() { return false; },
    toolCallAlreadyFinished() { return false; },
    hasChatItemForTool() { return false; },
    flushPendingTextBlock() {
      if (!context.pendingAssistantText) return;
      context.pendingAssistantBlocks.push({ type: 'text', text: context.pendingAssistantText });
      context.pendingAssistantText = '';
    },
    flushAssistantMessageToHistory() {
      context.flushPendingTextBlock();
      if (!context.pendingAssistantBlocks.length) return;
      state.messages.push({ role: 'assistant', content: context.pendingAssistantBlocks });
      context.pendingAssistantBlocks = [];
    },
    resetPendingAssistant() {},
    flushQueued: async () => {},
    enqueueBehindSubagentCompletionHold: async (_sid, enqueue) => enqueue(),
    renewSubagentCompletionHold: async () => false,
    startSubagentCompletionHoldHeartbeat() {},
    releaseSubagentCompletionHoldIfUnused: async () => false,
    isBusyFor() { return state.busy; },
    doSendFor: async () => {},
    ensureSessionBufferLoaded: async () => true,
    getBuffer(sessionId) { return sessionStates[sessionId] || null; },
    markRemoteTurn() {},
    reconcileRemoteTurn: async () => true,
    saveWorkingSetTo(buffer) {
      if (!buffer) return;
      buffer.busy = state.busy;
      buffer.messages = state.messages;
      buffer.chatItems = state.chatItems;
    },
    hydratedMessageKey() { return ''; },
    thinkingTool() {},
    thinkingIdle() {},
    startThinking() { state.thinking.active = true; },
    stopThinking() { state.thinking.active = false; },
    userMessageDisplayText() { return ''; },
    scheduleScheduledRunRefresh() {},
    handleMemoryWrite() {},
    isPresentArtifactTool() { return false; },
    artifactPathFromToolOutput() { return ''; },
    shouldUseToolOutputAsArtifact() { return false; },
    presentArtifactAbsPath() { return ''; },
    extractArtifactPaths() { return []; },
    fileMutationAction() { return ''; },
    markTurnDirtyArtifact() {},
    trackArtifact() {},
    untrackArtifact() {},
    findPresentedArtifact() { return null; },
    isDeliverable() { return false; },
    noteArtifactChange() {},
    publishRemoteLiveSnapshot: async () => false,
    persistMessagesFor: async () => {},
    composePlanMarkdown() { return ''; },
    refreshHistoryList: async () => {},
    isShellExecutionTool() { return false; },
    scheduleShellPoll() {},
    appendToolItemOutput() {},
    scheduleShellNotify() {},
    markBackgroundToolItem() {},
    patchLastItem() {},
    isDuplicateArtifactCard() { return false; },
    updateToolItem() { return null; },
    basename(value) { return String(value || ''); },
    hasUnresolvedItem() { return false; },
    finishBackgroundToolItem() {},
    safeConsoleInfo() {},
    isScheduledRunSession() { return false; },
    markScheduledInitialTurnTerminal() {},
    isAbsPath() { return false; },
    addOrMergePruneCompaction() {},
    toolResultDisplayContent(value) { return value; },
    currentStreamText: '',
    currentStreamId: 1,
    pendingAssistantText: '',
    pendingAssistantBlocks: [],
    itemIdSeq: 1,
    toolMeta: {},
  };

  const api = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'](context);
  return {
    api,
    state,
    context,
    timers,
    emit(name, payload = {}) {
      const handler = listeners.get(name);
      assert.ok(handler, `missing listener ${name}`);
      return handler({ event: name, payload: { session_id: 'session-1', ...payload } });
    },
    counts() {
      return { markdownCalls, deepNotifyCalls, streamingNotifyCalls };
    },
  };
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: 'A' });
  harness.emit('chat:delta', { text: 'B' });
  harness.emit('chat:delta', { text: 'C' });

  assert.equal(harness.timers.count(), 1, 'one session must own at most one cadence timer');
  assert.equal(harness.counts().markdownCalls, 0, 'tiny deltas must not render Markdown');
  assert.equal(harness.counts().deepNotifyCalls, 0, 'tiny deltas must not deep-publish state');
  assert.equal(harness.state.chatItems[0].text, 'A', 'the first chunk establishes the bubble synchronously');

  harness.emit('chat:tool_start', { id: 'tool-1', name: 'search', args: {} });
  assert.equal(harness.timers.count(), 0, 'a tool boundary must cancel the cadence timer');
  assert.equal(harness.state.chatItems[0].text, 'ABC', 'the boundary must flush every queued delta in order');
  assert.equal(harness.state.chatItems[0].html, '<md>ABC</md>');
  assert.equal(harness.state.chatItems[0].streaming, false);
  assert.equal(harness.state.chatItems[0].streamingPreviewText, undefined);
  assert.deepEqual(
    JSON.parse(JSON.stringify(harness.context.pendingAssistantBlocks)),
    [
      { type: 'text', text: 'ABC' },
      { type: 'tool_use', id: 'tool-1', name: 'search', input: {} },
    ],
    'the exact assistant block must precede the following tool block',
  );
  assert.deepEqual(harness.state.chatItems.map(item => item.type), ['assistant', 'tool']);
  assert.equal(harness.counts().markdownCalls, 1, 'the semantic boundary renders Markdown exactly once');
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: 'terminal-' });
  harness.emit('chat:delta', { text: 'answer' });
  harness.emit('chat:done', { status: 'Completed' });
  assert.equal(harness.timers.count(), 0, 'chat:done must synchronously clear the cadence timer');
  assert.equal(harness.state.chatItems[0].text, 'terminal-answer');
  assert.equal(harness.state.chatItems[0].html, '<md>terminal-answer</md>');
  assert.equal(harness.state.busy, false);
  assert.equal(harness.counts().markdownCalls, 1);

  harness.emit('chat:delta', { text: 'late transport tail' });
  assert.equal(harness.timers.count(), 0, 'a late post-terminal delta must not arm a timer');
  assert.equal(harness.state.chatItems[0].text, 'terminal-answer');
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: 'x' });
  for (let index = 0; index < 8192; index += 1) {
    harness.emit('chat:delta', { text: 'x' });
  }
  assert.equal(harness.state.chatItems[0].text.length, 8193,
    'an event-loop-starved stream must auto-flush at the 8 KiB pending waterline');
  assert.equal(harness.counts().streamingNotifyCalls, 1);
  assert.equal(harness.timers.count(), 0,
    'the waterline flush must cancel the starved cadence timer');
  assert.equal(harness.counts().markdownCalls, 0,
    'a waterline flush remains a plain React-text streaming update');
  harness.emit('chat:delta', { text: 'next' });
  assert.equal(harness.timers.count(), 1,
    'the next chunk starts a fresh bounded coalescing window');
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: '```persona-' });
  harness.emit('chat:delta', { text: 'card\n{"name":"Reviewer","body":"private' });
  assert.equal(harness.timers.fireNext(), true);
  assert.equal(harness.state.chatItems[0].streamingStructuredDraft, 'persona-card',
    'an explicit structured protocol marker split across deltas must be detected');
  assert.equal(harness.counts().markdownCalls, 0,
    'protocol detection must not restore per-cadence Markdown rendering');

  harness.emit('chat:tool_start', { id: 'tool-after-card', name: 'save', args: {} });
  assert.equal(harness.state.chatItems[0].streamingStructuredDraft, undefined,
    'the sticky live marker must be removed when the exact final Markdown is settled');
  assert.match(harness.state.chatItems[0].html, /persona-card/);
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: '```json\n{"bo' });
  harness.emit('chat:delta', { text: 'dy":"private instructions"' });
  assert.equal(harness.timers.fireNext(), true);
  assert.equal(harness.state.chatItems[0].streamingStructuredDraft, 'structured',
    'a generic fenced JSON shape hint split across deltas must activate the same placeholder');
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: '```scheduled-task-' });
  harness.emit('chat:delta', { text: 'draft\n{"name":"Daily brief","prompt":"prepare"' });
  assert.equal(harness.timers.fireNext(), true);
  assert.equal(harness.state.chatItems[0].streamingStructuredDraft, 'scheduled-task-draft');

  const sensitiveTail = `${'x'.repeat(20_000)}DO_NOT_SHOW_RAW_JSON"}\n\`\`\``;
  harness.emit('chat:delta', { text: sensitiveTail });
  assert.equal(harness.state.chatItems[0].streamingPreviewText.length, 16_384);
  assert.doesNotMatch(harness.state.chatItems[0].streamingPreviewText, /scheduled-task-draft/,
    'the protocol marker must be able to roll out of the bounded visual tail');
  assert.match(harness.state.chatItems[0].streamingPreviewText, /DO_NOT_SHOW_RAW_JSON/,
    'the stress case must place sensitive JSON inside the raw preview tail');
  assert.equal(harness.state.chatItems[0].streamingStructuredDraft, 'scheduled-task-draft',
    'the structured marker stays sticky after more than 16 KiB of JSON');
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
    // Force a visual cadence every 100 transport deltas. This is deliberately
    // more aggressive than a long-response 160 ms cadence and therefore a
    // useful allocation/regression stress case.
    if ((index + 1) % 100 === 0) {
      assert.equal(harness.timers.fireNext(), true);
      maxPreviewChars = Math.max(
        maxPreviewChars,
        harness.state.chatItems[0].streamingPreviewText.length,
      );
    }
  });
  harness.api.flushPendingChatDeltas('session-1', { finalize: true, notify: true });

  assert.equal(harness.state.chatItems[0].text.length, totalChars);
  assert.equal(harness.state.chatItems[0].text, expectedText, 'coalescing must preserve exact full text');
  assert.equal(maxPreviewChars, 16_384, 'live React text must have a hard 16 KiB character cap');
  assert.equal(harness.state.chatItems[0].streamingPreviewText, undefined);
  assert.equal(harness.state.chatItems[0].html, `<md>${expectedText}</md>`);
  assert.equal(harness.counts().markdownCalls, 1, '63,927 deltas still render Markdown only at terminal');
  assert.equal(harness.counts().deepNotifyCalls, 0, 'all streaming publications stay off deep clone');
  assert.ok(harness.counts().streamingNotifyCalls <= 640,
    '100-delta stress cadence must coalesce to at most 640 shallow publications');
  assert.equal(harness.timers.count(), 0, 'terminal flush must leave no timer behind');
  assert.ok(harness.timers.delays.includes(40) && harness.timers.delays.includes(160),
    'cadence must adapt from 40 ms to the bounded 160 ms long-answer interval');
}

{
  const harness = createHarness();
  harness.emit('chat:delta', { text: 'kept' });
  harness.emit('chat:delta', { text: '-pending' });
  assert.equal(harness.timers.count(), 1);
  assert.equal(harness.api.discardPendingChatDeltas('session-1'), true);
  assert.equal(harness.timers.count(), 0, 'session disposal must synchronously cancel its timer');
  assert.equal(harness.timers.fireNext(), false);
  assert.equal(harness.state.chatItems[0].text, 'kept', 'deleted-session pending text must not resurrect');
}

{
  const order = [];
  let releaseCancel;
  const cancelReply = new Promise(resolve => { releaseCancel = resolve; });
  const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  vm.runInContext(chatSource, vm.createContext({ window: windowObject, console }), {
    filename: 'chat.js',
  });
  const chat = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__.chat({
    state: { activeSessionId: 'session-1', busy: true },
    invoke(command, payload) {
      order.push(['invoke', command, payload]);
      return cancelReply;
    },
    safeConsoleInfo() {},
    flushPendingChatDeltas(sessionId, options) {
      order.push(['flush', sessionId, options]);
      return true;
    },
  });
  const cancellation = chat.cancelGeneration();
  assert.deepEqual(JSON.parse(JSON.stringify(order[0])), [
    'flush',
    'session-1',
    { finalize: true, notify: true },
  ], 'cancel must settle the stream before awaiting the backend RPC');
  assert.deepEqual(JSON.parse(JSON.stringify(order[1])), [
    'invoke',
    'cancel_generation',
    { sessionId: 'session-1' },
  ]);
  releaseCancel();
  await cancellation;
}

{
  const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  vm.runInContext(sessionsSource, vm.createContext({ window: windowObject, console, Date }), {
    filename: 'sessions.js',
  });
  const state = {
    activeSessionId: 'session-a',
    pendingDraftMultiAgent: false,
    messages: [], chatItems: [{ id: 1, type: 'assistant', text: 'visible' }], artifacts: [],
    composerDraft: '', turnTimeline: [], activeTurnTimelineId: null,
    personaEvents: [], pinvouReviews: [], pinvouSceneEvents: [], busy: true,
    planSnapshot: { plan: null, todos: null }, modeState: { mode: 'yolo' },
    thinking: { active: true }, tokens: { input: 0, max: 1000 }, queued: [],
    activePersona: null, mountedCollection: null, mountedCollections: [],
    mountedCollectionsRevision: 0, scheduledTaskDraft: null,
    turnDirtyArtifacts: [], turnPresentedArtifacts: [],
  };
  const sessionStates = {};
  const stream = {
    currentStreamText: 'visible', currentStreamId: 1,
    pendingAssistantText: 'visible', pendingAssistantBlocks: [],
    itemIdSeq: 1, toolMeta: {},
  };
  const sessionContext = {
    state,
    invoke: async () => null,
    notify() {},
    sessionStates,
    scheduledRunSessionOwners: {},
    personaPlaceholderTitles: {},
    turnUsageDirty: {},
    flushPendingChatDeltas(sessionId) {
      assert.equal(sessionId, 'session-a');
      state.chatItems.push({ id: 2, type: 'assistant', text: 'pending-flushed' });
      stream.currentStreamText = 'visiblepending-flushed';
      return true;
    },
    discardPendingChatDeltas() {},
    filterSessionArtifacts(value) { return value; },
    scheduleShellPoll() {},
    isScheduledRunSession() { return false; },
    get currentStreamText() { return stream.currentStreamText; },
    set currentStreamText(value) { stream.currentStreamText = value; },
    get currentStreamId() { return stream.currentStreamId; },
    set currentStreamId(value) { stream.currentStreamId = value; },
    get pendingAssistantText() { return stream.pendingAssistantText; },
    set pendingAssistantText(value) { stream.pendingAssistantText = value; },
    get pendingAssistantBlocks() { return stream.pendingAssistantBlocks; },
    set pendingAssistantBlocks(value) { stream.pendingAssistantBlocks = value; },
    get itemIdSeq() { return stream.itemIdSeq; },
    set itemIdSeq(value) { stream.itemIdSeq = value; },
    get toolMeta() { return stream.toolMeta; },
    set toolMeta(value) { stream.toolMeta = value; },
  };
  const sessions = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__.sessions(sessionContext);
  sessionStates['session-a'] = sessions.freshBuffer();
  sessionStates['session-b'] = sessions.freshBuffer();
  sessions.switchActiveTo('session-b');
  assert.equal(state.activeSessionId, 'session-b');
  assert.ok(sessionStates['session-a'].chatItems.some(item => item.text === 'pending-flushed'),
    'switching sessions must flush pending deltas before saving the old working set');
}

assert.match(
  bridgeSource,
  /function notifyStreamingChat\(\)[\s\S]*?publishSubscribers\(true\)/,
  'the production hot notification path must use shallow snapshots',
);
assert.match(
  bridgeSource,
  /field === "chatItems"[\s\S]*?state\[field\]\.slice\(\)/,
  'hot snapshots must change chatItems array identity for memoized consumers',
);
assert.match(
  chatViewSource,
  /streamingPreviewText[\s\S]*?whitespace-pre-wrap[\s\S]*?\{streamingPreview\}/,
  'the normal and unified chat bubble must render the live tail as a React text node',
);
assert.match(
  chatViewSource,
  /streamingStructuredDraft \? \([\s\S]*?streamingDraftLabel[\s\S]*?\) : streamingPreview \? \(/,
  'ChatView must render the structured placeholder before considering the raw preview tail',
);
assert.match(
  voiceShellSource,
  /streamingPreviewText[\s\S]*?pinvou-os-answer-stream-text[\s\S]*?visibleAssistant\.streamingPreviewText/,
  'VoiceShell must render the same bounded React text tail',
);
assert.match(
  voiceShellSource,
  /visibleAssistant\.streamingStructuredDraft \? \([\s\S]*?draftingScheduled[\s\S]*?cpDesigning[\s\S]*?\) : visibleAssistant\.streaming && visibleAssistant\.streamingPreviewText \? \(/,
  'VoiceShell must render the structured placeholder before considering the raw preview tail',
);
assert.match(
  chatEventsSource,
  /STREAMING_STRUCTURED_PROBE_CHAR_LIMIT = 1024[\s\S]*?offset \+= STREAMING_STRUCTURED_PROBE_CHAR_LIMIT/,
  'structured protocol detection must scan only bounded slices of newly arrived text',
);
assert.doesNotMatch(
  voiceShellSource,
  /import\s+\{\s*renderMarkdown\s*\}|renderMarkdown\(/,
  'VoiceShell must reuse terminal item.html instead of reparsing the full answer',
);

console.log('tauri_stream_backpressure: ok');
