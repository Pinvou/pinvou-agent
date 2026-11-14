#!/usr/bin/env node
// Contract tests for the main lane's manual compact entry (the ctx usage
// row button):
//   1. Behavior layer (a vm-driven tauri chat-events listener):
//      chat:compaction start→terminal maintains the
//      state.compactingSessions in-flight marker, and done's post_tokens —
//      a conservative post-compaction usage estimate the forwarder gained
//      for the usage bar — is consumed ahead of the dirty guard.
//   2. Source-contract layer: the ChatView button guard (busy/compacting/
//      per-session enqueue), the session_engine_not_running localized
//      mapping, the mirrored web/tauri bodies, and exactly one i18n entry
//      per language.
// Reuses the vm harness from chat_usage_context_window.test.mjs.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => readFileSync(path.join(root, ...parts), 'utf8');

// ── tauri behavior layer: the compactingSessions marker + post_tokens ──
const source = read('src', 'shared', 'bridge-shared-helpers.js') + '\n' + read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
vm.runInContext(source, vm.createContext({
  window: windowObject,
  console,
  Date,
  String,
}), { filename: 'chat-events.js' });

const listeners = new Map();
let notifyCount = 0;
const pruneMerges = [];
const state = {
  activeSessionId: 'session-1',
  chatItems: [],
  messages: [],
  tokens: { input: 40000, max: 131072 },
  thinking: { active: false },
  compactingSessions: {},
  compactionStarts: {},
  compactionSeq: 0,
};
const context = {
  state,
  listen(name, handler) { listeners.set(name, handler); },
  notify() { notifyCount += 1; },
  invoke: async () => null,
  turnUsageDirty: {},
  sessionStates: {},
  renderMarkdown(text) { return `<p>${text}</p>`; },
  bt() { return ''; },
  onSessionEvent(_event, callback) { callback(); },
  runSyncOnSession(_sessionId, callback) { callback(); },
  addChatItem(item) {
    item.id = ++context.itemIdSeq;
    state.chatItems.push(item);
  },
  addSystemItem(text, meta = {}) {
    context.addChatItem({ type: 'system', text, ...meta });
  },
  addOrMergePruneCompaction(compactId) { pruneMerges.push(compactId); },
  timeStr() { return '12:00'; },
  flushPendingTextBlock() {
    if (!context.pendingAssistantText) return;
    context.pendingAssistantBlocks.push({ type: 'text', text: context.pendingAssistantText });
    context.pendingAssistantText = '';
  },
  currentStreamId: 1,
  currentStreamText: '',
  pendingAssistantText: '',
  pendingAssistantBlocks: [],
  itemIdSeq: 1,
};
windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'](context);

const emitCompaction = payload => listeners.get('chat:compaction')({
  event: 'chat:compaction',
  payload: { session_id: 'session-1', ...payload },
});

// Scenario 1: start sets a timestamped in-flight marker (manual and auto
// compactions share the event stream; the read side heals after 15 minutes
// off the timestamp — see ChatView and the eviction-protection predicate).
emitCompaction({ phase: 'start', id: 'compact-1', auto: false, message: 'Manual context compaction started' });
const compactStartedAt = state.compactingSessions['session-1'];
assert.ok(typeof compactStartedAt === 'number' && compactStartedAt > 0, 'start must set a timestamped compaction in-flight marker');
assert.equal(state.compactionStarts['session-1'], 1, 'start must also record the persistent per-session start signal, carrying the bridge\'s monotonic compaction sequence (not a wall-clock time — review 5876783763)');
assert.equal(state.chatItems.length, 1, 'the start system item is listed as usual');

// Scenario 2: done clears the marker, and post_tokens is the
// post-compaction usage estimate — it refreshes the usage bar directly
// (keeping the real window denominator) instead of waiting for the next
// usage event.
const notifyAfterStart = notifyCount;
emitCompaction({ phase: 'done', id: 'compact-1', messages_before: 12, messages_after: 8, post_tokens: 9500, message: '12 → 8 messages' });
assert.equal(state.compactingSessions['session-1'], undefined, 'the done terminal must clear the in-flight marker');
assert.equal(state.compactionStarts['session-1'], 1, 'the start signal PERSISTS after the terminal (a coalesced start+terminal for a background session must stay reconcilable)');
assert.equal(state.tokens.input, 9500, 'post_tokens must refresh the usage bar input');
assert.equal(state.tokens.max, 131072, 'the real window denominator is unchanged');
assert.ok(notifyCount > notifyAfterStart, 'the marker clear/usage refresh must notify the UI');

// Scenario 3: the post_tokens guard — missing / non-positive / over-window
// values must not pollute the usage bar (the last accurate value stays).
for (const bad of [undefined, 0, -5, 999999]) {
  emitCompaction({ phase: 'start' });
  const inputBefore = state.tokens.input;
  emitCompaction({ phase: 'done', messages_before: 8, messages_after: 8, post_tokens: bad, message: 'x' });
  assert.equal(state.tokens.input, inputBefore, `post_tokens=${bad} must not update the usage bar`);
}

// Scenario 3b: boundary — post_tokens exactly at the window cap must be
// accepted (the guard is <=, not <; a post-compaction usage sitting at the
// window edge is a legal state, and switching to < would reject it).
emitCompaction({ phase: 'start' });
emitCompaction({ phase: 'done', messages_before: 8, messages_after: 8, post_tokens: 131072, message: 'x' });
assert.equal(state.tokens.input, 131072, 'post_tokens exactly at the window cap must refresh the usage bar');

// Scenario 4: fail/cancel terminals clear the marker too — the entry must
// not stay disabled forever.
for (const phase of ['fail', 'cancel']) {
  emitCompaction({ phase: 'start' });
  assert.ok(state.compactingSessions['session-1'] > 0);
  emitCompaction({ phase, message: 'x' });
  assert.equal(state.compactingSessions['session-1'], undefined, `the ${phase} terminal must clear the in-flight marker`);
}

// Scenario 5: a prune-only auto compaction (the done+pruneOnlyAuto merge
// path) clears the marker as well.
emitCompaction({ phase: 'start' });
emitCompaction({ phase: 'done', auto: true, messages_before: 8, messages_after: 8, message: '2 tool results pruned' });
assert.equal(state.compactingSessions['session-1'], undefined, 'the prune-only merge path must clear the marker too');
assert.equal(pruneMerges.length, 1, 'the prune-only merge path is unaffected');

// Scenario 6: a malformed event without session_id — it must not write a
// marker and must not blow up the listener.
assert.doesNotThrow(() => emitCompaction({ phase: 'start', session_id: undefined }));
assert.equal(Object.keys(state.compactingSessions).length, 0, 'a missing session_id must not write a marker');

// Scenario 7: usage events during a compaction turn stay frozen by the
// dirty guard (post_tokens already carried the usage estimate, so
// subsequent accumulated input is untrustworthy — the same semantics as
// chat_usage_context_window's dirty scenario). The dirty flag must be set
// by the compaction event itself (the listener self-marks) and the test
// must not pre-seed it — otherwise deleting the self-mark leaves this
// scenario green and the post_tokens-bypasses-dirty premise loses its
// pin.
const emitUsage = payload => listeners.get('chat:usage')({
  event: 'chat:usage',
  payload: { session_id: 'session-1', ...payload },
});
emitCompaction({ phase: 'start', session_id: 'session-2' });
assert.ok(context.turnUsageDirty['session-2'], 'the compaction event must set turnUsageDirty itself (the post_tokens-bypasses-dirty premise)');
emitUsage({ session_id: 'session-2', input_tokens: 40000 });
assert.equal(state.tokens.input, 131072, 'accumulated input during a compaction turn is untrustworthy; the last accepted post_tokens value stays');

// ── web/tauri source contract: the compaction handler must maintain the
// marker inline and consume post_tokens ──
const tauriCompaction = source.slice(source.indexOf('listen("chat:compaction"'));
const webSource = read('src', 'platform', 'web', 'bridge.js');
const webCompaction = webSource.slice(webSource.indexOf('listen("chat:compaction"'));
for (const [label, section] of [['tauri', tauriCompaction], ['web', webCompaction]]) {
  assert.ok(section.includes('state.compactingSessions[sid] = Date.now();'), `${label}: start must set a timestamped marker (the read side heals off it)`);
  assert.ok(section.includes('else delete state.compactingSessions[sid];'), `${label}: the terminal must clear the marker via the else branch (done/fail/cancel all covered)`);
  assert.ok(section.includes('state.tokens = { input: postTokens, max: state.tokens.max };'), `${label}: post_tokens must be written into the usage bar`);
  assert.ok(section.includes('postTokens <= state.tokens.max'), `${label}: the post_tokens over-window guard is retained`);
  assert.ok(section.includes('The fallback reads the wall clock'), `${label}: the wall-clock fallback tradeoff must stay documented (the early re-enable window is an accepted cost, kept so it is not mistaken for a bug and "fixed")`);
  // The marker-maintenance block's explicit notify must be pinned: done's
  // post_tokens branch and addSystemItem each notify on their own, so no
  // count assertion can tell which one notified — mutants that deleted this
  // block's notify (start set / terminal clear stopped refreshing
  // subscribers; both the button disable and the takeover effect went dead)
  // were green before, so the pin is by block text.
  assert.ok(section.includes('if (sid) {\n      if (phase === "start") {\n        state.compactingSessions[sid] = Date.now();\n        state.compactionSeq = (state.compactionSeq || 0) + 1;\n        state.compactionStarts[sid] = state.compactionSeq;\n      } else delete state.compactingSessions[sid];\n      notify();\n    }'),
    `${label}: the marker maintenance must notify explicitly (subscriber refreshes rely on it; the post_tokens branch's notify does not cover this branch — deleting it used to be green)`);
}
// The both-sides-must-move-together constraint, stated hard: from the
// listen marker to the post_tokens block tail the two registration bodies
// must be byte-identical — web-side drift no longer relies only on the
// indirect coverage of the four string pins above.
const compactionSpanTail = '        notify();\n      }\n    }';
const spanToPostTokensTail = section => section.slice(0, section.indexOf(compactionSpanTail) + compactionSpanTail.length);
assert.equal(spanToPostTokensTail(tauriCompaction), spanToPostTokensTail(webCompaction),
  'the tauri/web compaction registration bodies must share a byte-identical common prefix (marker maintenance + post_tokens consumption move together)');

// ── bridge state wiring: the field joins the slice, both state
// declarations, deletion/eviction clear it, eviction protection ──
const sharedSource = read('src', 'shared', 'bridge-shared-helpers.js');
assert.ok(sharedSource.includes('"compactingSessions"'), 'STATE_SLICE_FIELDS.chat must declare compactingSessions (both lanes share the constant in their snapshots)');
assert.ok(sharedSource.includes('"compactionStarts"'), 'STATE_SLICE_FIELDS.chat must declare compactionStarts (the start signal must reach the UI snapshot)');
assert.ok(sharedSource.includes('Date.now() - state.compactingSessions[id] < 15 * 60 * 1000'), 'capacity-eviction protection must read the in-flight marker against the fallback bound (an expired marker does not protect)');
const tauriBridgeSource = read('src', 'platform', 'tauri', 'bridge.js');
assert.equal(tauriBridgeSource.split('compactingSessions: {}').length - 1, 1, 'the tauri state must declare compactingSessions');
assert.equal(tauriBridgeSource.split('compactionStarts: {}').length - 1, 1, 'the tauri state must declare compactionStarts');
assert.equal(webSource.split('compactingSessions: {}').length - 1, 1, 'the web state must declare compactingSessions');
assert.equal(webSource.split('compactionStarts: {}').length - 1, 1, 'the web state must declare compactionStarts');
const tauriSessionsSource = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
assert.equal(tauriSessionsSource.split('delete state.compactingSessions[id]').length - 1, 3, 'the tauri deletion/eviction sites must clear the marker too (all three)');
assert.equal(tauriSessionsSource.split('delete state.compactionStarts[id]').length - 1, 3, 'the tauri deletion/eviction sites must clear the start signal too (all three)');
assert.equal(webSource.split('delete state.compactingSessions[id]').length - 1, 3, 'the web deletion/eviction sites must clear the marker too (all three)');
assert.equal(webSource.split('delete state.compactionStarts[id]').length - 1, 3, 'the web deletion/eviction sites must clear the start signal too (all three)');

// ── ChatView source contract: the guard, per-session enqueue, the error
// mapping, and the testability hooks ──
const chatViewSource = read('src', 'features', 'chat', 'ChatView.jsx').replace(/\r\n/g, '\n');
assert.match(chatViewSource, /if \(!sid \|\| manualCompactActive \|\| compactingActive \|\| busy\) return;/,
  'the entry guard must block both busy and an in-flight compaction, or a mid-turn click is a silent no-op and a post-reset click enqueues twice');
// The command only enqueues and returns: sid must be captured at the call
// site (an error after a session switch still lands on the original
// session); the manual marker must be set BEFORE the invoke and right next
// to the try — between the enqueue and the bridge's start takeover the
// button must not reset. The admission object is captured in a variable:
// it IS the request's generation, and the failure path clears by reference
// equality so an older rejection cannot erase a newer request's marker for
// the same session (zhuowp round-3 P2).
assert.match(chatViewSource, /const sid = activeSessionId;\s*if \(!sid[\s\S]*?const seqAt = compactionSeq;[\s\S]*?const admission = \{ at: Date\.now\(\), seqAt, itemsAt: maxItemId \};[\s\S]*?setManualCompactBySid\(current => \(\{ \.\.\.current, \[sid\]: admission \}\)\);\s*try \{/, 'sid must be captured before the invoke, and the admission generation object (with the sequence snapshot and item-id snapshot) set right next to it');
assert.match(chatViewSource, /invokeTauri\('compact_now', \{ sessionId: sid \}\)/, 'the compaction command and its argument shape');
// Per-session keying (ChatView does not remount on a session switch): a
// boolean would disable B while A's enqueue is in flight.
// The state is SEEDED FROM the module store and written through to it
// (zhuowp round-3 P2): main.jsx renders ChatView only for the chat view, so
// a settings round-trip unmounts it — plain state would discard the
// admission while its queued Web RPC lives on, and the remounted view would
// re-enable the button over the in-flight request.
assert.match(chatViewSource, /const \[manualCompactBySid, setManualCompactBySidState\] = useState\(manualCompactAdmissionStore\.map\);/, 'the manual enqueue state must be seeded from the module store so a remount inherits the pending admission');
assert.match(chatViewSource, /const setManualCompactBySid = \(updater\) => setManualCompactBySidState\(\(current\) => \{\s*const next = typeof updater === 'function' \? updater\(current\) : updater;\s*manualCompactAdmissionStore\.map = next;\s*return next;\s*\}\);/, 'every update must write through to the module store (the render mirror alone dies with the component)');
assert.match(chatViewSource, /const manualCompactAdmissionStore = \{ map: \{\} \};/, 'the admission store must live at module scope (survives ChatView unmount/remount)');
assert.match(chatViewSource, /compactingStartedAt && Date\.now\(\) - compactingStartedAt < COMPACTING_FLAG_MAX_AGE_MS/, 'the bridge in-flight marker must heal after 15 minutes (mirroring MAX_COMPACTION_LATCH_AGE)');
// The heal timer schedules only STILL-FRESH boundaries (zhuowp round-3 P3):
// an already-expired bridge timestamp winning the min would fire once
// immediately and leave the fresh manual marker's expiry unscheduled — the
// idle view then never re-enables at the real boundary, and re-running the
// effect cannot repair it (its deps are unchanged by the tick).
assert.match(chatViewSource, /const boundaries = \[\];\s*if \(compactingStartedAt && now - compactingStartedAt < COMPACTING_FLAG_MAX_AGE_MS\) \{\s*boundaries\.push\(compactingStartedAt \+ COMPACTING_FLAG_MAX_AGE_MS\);\s*\}\s*if \(activeManualCompact && now - activeManualCompact\.at < COMPACTING_FLAG_MAX_AGE_MS\) \{\s*boundaries\.push\(activeManualCompact\.at \+ COMPACTING_FLAG_MAX_AGE_MS\);\s*\}\s*if \(!boundaries\.length\) return;/, 'the heal timer must consider only still-fresh boundaries (an expired bridge timestamp must not preempt the fresh manual marker\'s expiry)');
assert.match(chatViewSource, /const activeManualCompact = activeSessionId \? manualCompactBySid\[activeSessionId\] : undefined;/, 'the active session reads its OWN marker from the map (review 5874108926 P2: per-sid retention, not a single slot)');
assert.match(chatViewSource, /manualCompactActive = !!\(activeManualCompact\s*&& Date\.now\(\) - activeManualCompact\.at < COMPACTING_FLAG_MAX_AGE_MS\)/, 'the manual marker is bound by the same fallback TTL');
assert.ok(chatViewSource.includes('const COMPACTING_FLAG_MAX_AGE_MS = 15 * 60 * 1000'),
  'the ChatView fallback bound must stay pinned to Rust MAX_COMPACTION_LATCH_AGE (15 minutes)');
assert.match(chatViewSource, /setManualCompactBySid\(current => \{[\s\S]*?if \(current\[sid\] !== admission\) return current;[\s\S]*?const next = \{ \.\.\.current \};\s*delete next\[sid\];\s*return next;\s*\}\);/, 'a failure releases only THIS request\'s admission (reference-equality generation: a newer same-session request that overwrote the slot must survive an older rejection — zhuowp round-3 P2)');
// Once the bridge's start marker lands for a session, its own manual
// marker must be released immediately (identity-checked, per session);
// the release must not be conditioned on the session being active — when
// a compaction finishes in the background the effect never sees
// compactingActive, and the leftover marker would keep the button
// disabled for another TTL showing a false "compacting" state.
// The takeover must also require the marker to be FRESH: an entry whose
// terminal was lost lingers until deletion/eviction, and a bare existence
// check would let such a stale entry instantly release a retry click's
// brand-new marker — the entry resets before the
// new start lands and a fast double click enqueues twice.
// The takeover has two independent release paths, both pinned:
// (a) the bridge marker exists and is fresh — the regular path; a stale
//     entry must not take over (releasing the retry marker early would
//     reopen the double-click window), independent of session activation
//     (background-finished compactions).
assert.match(chatViewSource, /for \(const sid of Object\.keys\(current\)\)/, 'the reconcile sweep walks EVERY pending marker (review 5874108926 follow-up: background sessions reconcile too, not just the active one)');
assert.match(chatViewSource, /if \(now - current\[sid\]\.at >= COMPACTING_FLAG_MAX_AGE_MS\) \{ drop\(sid\); continue; \}/, 'TTL release drops expired entries for every session (not just the active one)');
assert.match(chatViewSource, /const startedSeq = startsMap \? startsMap\[sid\] : 0;/, 'the reconcile reads the session\'s last-start sequence');
assert.match(chatViewSource, /if \(startedSeq > current\[sid\]\.seqAt\) \{ drop\(sid\); \}/, 'a start with a sequence STRICTLY greater than the click snapshot releases the marker — even when the session is NOT active and the terminal coalesced with the start (A starts+completes while B is active); strictness plus the monotonic counter makes a prior same-millisecond start unable to clear a new marker');
assert.ok(!chatViewSource.includes('if (flagAt && now - flagAt < COMPACTING_FLAG_MAX_AGE_MS) drop(sid);'), 'the fresh-flag arm is subsumed by the sequence arm (a fresh flag implies a start landed, i.e. a strictly greater sequence) — the dead arm must not return');
assert.ok(!chatViewSource.includes('if (sid !== activeSessionId)'), 'the reconcile sweep must NOT skip non-active sessions — an active-only guard would strand background markers until TTL (the appearance-only pin above cannot see an if-false disable, so its absence is pinned directly)');
assert.match(chatViewSource, /const startsMap = bs && bs\.compactionStarts;/, 'ChatView must read the persistent per-session start signal (the bridge survives terminals and prune-only merges)');
assert.match(chatViewSource, /const compactionSeq = \(bs && bs\.compactionSeq\) \|\| 0;/, 'ChatView must read the bridge\'s monotonic compaction sequence (review 5876783763: the click snapshots it, the reconcile compares sequence numbers, not wall-clock times)');
assert.match(chatViewSource, /for \(const sid of Object\.keys\(current\)\)/, 'the sweep consults the GLOBAL maps for ALL sessions (each sid\'s own start signal), not an active-session lookup');
// (b) the start system-item fallback — when start and the terminal arrive
//     in the same React batch (an instantly failing compaction), the bridge
//     marker never appeared in any render and path (a) can never see it; start system items
//     is an append-only projection, so the post-batch
//     snapshot still contains it. All three ingredients are required:
assert.match(chatViewSource, /itemsAt: maxItemId/, 'the click must snapshot the item-id upper bound (defining "start items newer than the click")');
assert.match(chatViewSource, /item\.id <= entry\.itemsAt/, 'the start-item fallback must be bounded by the click snapshot (an old start must not release a new marker)');
assert.match(chatViewSource, /item\.type === 'system' && item\.compactPhase === 'start'/, 'the start system-item fallback release must be pinned (with start and the terminal in one batch the bridge marker never appears, so only this can release)');
// The release effect's dependency array must be pinned: without
// compactingMap a bridge-marker change is missed, without
// manualCompactBySid the effect closes over a stale map and clears the
assert.match(chatViewSource, /\}, \[compactingMap, startsMap, manualCompactBySid, chatItems, activeSessionId\]\);/, 'the release effect must declare the [compactingMap, startsMap, manualCompactBySid, chatItems, activeSessionId] deps (a stale startsMap misses background starts)');
// The fallback heal must be proactive: while the entry is disabled, a tick
// is scheduled at the expiry boundary so a completely idle app (no
// unrelated notify, and a click on a disabled button produces no render)
assert.match(chatViewSource, /if \(!ctxCompactInFlight\) return;[\s\S]{0,1000}setCompactHealTick\(current => current \+ 1\)/, 'a fallback expiry tick must be scheduled while the entry is disabled (the heal cannot rely on passive re-renders)');
assert.match(chatViewSource, /const healDelayMs = Math\.min\(\.\.\.boundaries\) - now;/, 'the fallback tick must take the earliest FRESH boundary (bridge and manual marker; already-expired boundaries are not schedulable — zhuowp round-3 P3)');
// The in-flight derivation must include both sources and its definition
// must be pinned (every pin references the names; without pinning the
// definition, a mutant narrowing it to either source alone is green —
assert.match(chatViewSource, /const ctxCompactInFlight = compactingActive \|\| manualCompactActive;/, 'the in-flight derivation must include both the bridge and manual markers (either alone silently misses the other window)');
// Both fallback boundary operands must keep the : Infinity guard: without
// it Math.min reads null.at once the marker is released (the normal state
// after a takeover), throwing a TypeError that crashes the whole ChatView
assert.match(chatViewSource, /if \(compactingStartedAt && now - compactingStartedAt < COMPACTING_FLAG_MAX_AGE_MS\) \{[\s\S]{0,200}activeManualCompact && now - activeManualCompact\.at < COMPACTING_FLAG_MAX_AGE_MS\) \{[\s\S]{0,200}if \(!boundaries\.length\) return;/, 'both fresh-boundary operands must keep their truthiness guards plus the empty-collection early return (the released-marker normal state must not dereference null.at — zhuowp round-3 P3)');
// Timer hygiene: the cleanup and the clamp must be pinned (a mutant
// deleting clearTimeout used to be green).
assert.match(chatViewSource, /const healTimer = setTimeout\(/, 'the fallback tick must go through a clearable timer handle');
assert.match(chatViewSource, /return \(\) => clearTimeout\(healTimer\);/, 'the fallback timer must be cleaned up with the effect (no timer may outlive the in-flight window)');
assert.match(chatViewSource, /const healDelayMs = Math\.min\(\.\.\.boundaries\) - now;\s*if \(healDelayMs <= 0\) return;/, 'the expiry delay must be provably positive before scheduling (only fresh boundaries are collected and a non-positive delay returns instead of scheduling — the clock-jump clamp, zhuowp round-3 P3)');
// The in-flight copy and the hint tip must actually reach the button (the
// i18n keys must not become dead copy).
assert.match(chatViewSource, /title=\{ctxCompactInFlight \? t\.compactingNow : t\.compactNowTip\}/, 'compactingNow/compactNowTip must actually be used by the button');
// The visible content defaults to the usage readout (ctxUsageText is its
// own value, shared by the screen-reader name and the visible content),
// and the screen-reader name must be the composed "readout + action name": an
// aria-label overwrite loses the usage readout (round-6 review M-5); in
assert.match(chatViewSource, /const ctxUsageText = ctxTokens && ctxTokens\.max > 0/, 'the usage readout text must be its own value (shared by visible content and the screen-reader name)');
assert.match(chatViewSource, /aria-label=\{ctxCompactInFlight \? t\.compactingNow : `\$\{ctxUsageText\}, \$\{t\.compactNowTip\}`\}/, 'the screen-reader name must be the usage readout + action name (an action-only label loses the usage readout for screen readers)');
assert.match(chatViewSource, /\{ctxCompactInFlight \? t\.compactingNow : ctxUsageText\}/, 'the in-flight visible copy must switch with the state (a mutant deleting the switch used to be green)');
// The lazy-engine error mapping: the raw error chain must not surface
// (same as the code lane's nativeCompactInactive).
assert.match(chatViewSource, /raw\.includes\('session_engine_not_running'\) \? t\.uiCodex\.nativeCompactInactive : raw/, 'an engine-not-running error must map to the localized copy');
assert.ok(!chatViewSource.includes('compactFailNotice'), 'the failure copy must reuse uiCodex.compactFail, not introduce a duplicate key');
// The failure feedback's DELIVERY must be pinned: flashQueuedNotice is the
// feature's only error feedback surface, and deleting the whole call used
// to leave every test green (the mapping-expression pin does not cover delivery).
assert.match(chatViewSource, /flashQueuedNotice\(sid, \{ text: `\$\{t\.uiCodex\.compactFail\}: \$\{detail\}` \}\);/, 'a failure must be delivered through flashQueuedNotice into the shared queue (deleting the call must not pass silently)');
// UI hooks and the disabled shape.
assert.match(chatViewSource, /onClick=\{handleManualCompact\}/, 'the button must actually wire the click handler (removing onClick leaves every other pin green)');
assert.match(chatViewSource, /data-testid="chat-ctx-compact"/, 'the button must carry a testid (for DOM/e2e targeting, not text-dependent)');
assert.match(chatViewSource, /disabled=\{busy \|\| ctxCompactInFlight\}/, 'the disabled shape must include busy and an in-flight compaction');

// A→B→A retention (review 5874108926 P2): queue A, switch to B and queue
// there before A's bridge start arrives, then switch back to A — A's
// pending marker must still be retained so the guard blocks a second
// enqueue (the pool does not coalesce CompactContext operations, and an
// immediate second compaction is an extra lossy summary). Three shapes
// together provide that coverage:
// (i) the click's setState SPREADS the previous map — a B-side set cannot
//     erase A's entry (a single slot, or a fresh `{ [sid]: ... }`, would);
// (ii) the active-session reads key off activeSessionId (pinned above);
// (iii) both release paths delete ONLY the matching sid's key — a B-side
//     start or failure must not release A's entry.
assert.match(chatViewSource, /const seqAt = compactionSeq;/, 'A→B→A (i): the click snapshots the compaction sequence (review 5876783763: replaces wall-clock ordering)');
assert.match(chatViewSource, /setManualCompactBySid\(current => \(\{ \.\.\.current, \[sid\]: admission \}\)\);/, 'A→B→A (i): an enqueue must spread the previous map so another session\'s pending marker survives (the slot value is the request\'s generation object — zhuowp round-3 P2)');
assert.match(chatViewSource, /const drop = sid => \{\s*if \(next === current\) next = \{ \.\.\.current \};\s*delete next\[sid\];\s*\};/, 'A→B→A (iii): every release path drops only the matching sid\'s key from a copied map, never other sessions\' pending markers');

// ── i18n: exactly one entry per locale (count-1 per language); duplicate keys must not regress ──
for (const locale of ['en', 'zh', 'ja']) {
  const dictSource = read('src', 'shared', 'i18n', `${locale}.js`);
  for (const key of ['compactNowTip', 'compactingNow']) {
    assert.equal(dictSource.split(`${key}:`).length - 1, 1, `${locale}: ${key} must exist exactly once`);
  }
  assert.ok(!dictSource.includes('compactFailNotice'), `${locale}: must not introduce a duplicate key alongside uiCodex.compactFail`);
}
