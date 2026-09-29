/**
 * Aux chat panel JSX contract: the shape pins that remain after the async
 * state machine moved into aux-chat-controller.mjs (round-30 B1) and gained
 * executing coverage in aux_chat_controller.test.mjs. This file pins only
 * what executing tests cannot see: the panel adapter's wiring (singleton
 * controller, panel lifecycle, event guards), the rendering of the
 * controller's view state, the scroll/composer effects that stayed in the
 * JSX, and the aux entry gates on ChatView / CodexAcpView plus the quote
 * render branch on ConversationTimeline. Behavior pins with executing
 * coverage were DELETED, not moved (see ui_language_coverage.test.mjs
 * history); anything pinned here is JSX shape by construction.
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');
// Same strip mechanism ui_language_coverage uses: shape pins must not match
// text inside comments (a commented-out guard would otherwise still satisfy
// every anchor). Negative pins run against the RAW text.
const stripComments = text => text
  .replace(/\/\*[\s\S]*?\*\//g, comment => comment.replace(/[^\n]/g, ''))
  .split('\n')
  .map(line => line.replace(/\/\/.*$/, ''))
  .join('\n');

const auxChatPanel = stripComments(source('features/aux-chat/AuxChatPanel.jsx'));
const auxChatPanelRaw = source('features/aux-chat/AuxChatPanel.jsx');
const controller = stripComments(source('features/aux-chat/aux-chat-controller.mjs'));
const chat = stripComments(source('features/chat/ChatView.jsx'));
const conversation = stripComments(source('features/conversation/ConversationTimeline.jsx'));
const codex = stripComments(source('features/codex/CodexAcpView.jsx'));
const auxQuoteSelection = stripComments(source('features/aux-chat/AuxQuoteSelection.jsx'));

// ── Adapter wiring: the JSX is a thin shell over the controller singleton ──

test('the controller is a module-scope singleton shared by every mounted panel', () => {
  // The controller is a module-scope singleton ON PURPOSE: the registries
  // track backend-scoped operations that outlive any panel instance, so all
  // mounted panels must share one controller.
  assert.match(auxChatPanel, /const auxChatController = createAuxChatController\(\);/);
  assert.match(auxChatPanel, /import \{ createAuxChatController, reconcileLiveTaskIds, removeAuxQuote \} from '\.\/aux-chat-controller\.mjs';/);
});

test('M5: per-task registries and staged quotes purge on session deletion', () => {
  // M5: the per-task registries (restart epochs, unsent drafts) and staged
  // quotes purge when the sessions domain reports the task deleted — wired
  // once at module scope, armed from the first bind effect. Two legs: the
  // sessions-slice diff (chat tasks) and the session:deleted event (every
  // id — list_sessions excludes code-mode sessions, so the diff leg alone
  // never learns a code task was deleted, round-33 MAJOR-3).
  assert.match(auxChatPanel, /wireAuxSessionPurge\(\);/);
  assert.match(auxChatPanel, /bridge\.state\.subscribeMany\(\['sessions'\]/);
  assert.match(auxChatPanel, /reconcileLiveTaskIds\(knownTaskIds, liveTaskIds/);
  assert.match(auxChatPanel, /auxChatController\.purgeTask\(taskId\)/);
  assert.match(auxChatPanel, /bridge\.sessions\.onSessionDeleted/);
  assert.match(auxChatPanel, /knownTaskIds\.delete\(id\)/);
  assert.match(auxChatPanel, /auxChatController\.purgeTask\(id\)/);
});

test('the armed New Topic visible label is the short confirm, not the full sentence (round-35 MAJOR-3)', () => {
  // The full destructive sentence overflowed the 420 px dock minimum; the
  // short key is the VISIBLE span while the complete copy stays in the
  // title/aria. Pin the consumption, not just the dictionaries: deleting
  // newTopicConfirmShort must fail here AND in ui_language_coverage.
  assert.match(
    auxChatPanel,
    /\{view\.restartArmed \? copy\.newTopicConfirmShort : copy\.newTopic\}/,
    'the armed visible label must consume newTopicConfirmShort',
  );
  assert.match(
    auxChatPanel,
    /title=\{view\.restartArmed \? copy\.newTopicConfirm : copy\.newTopic\}/,
    'the full destructive copy stays in the title attribute',
  );
});

test('one controller panel per mounted instance, mirrored into state and disposed on unmount', () => {
  assert.match(auxChatPanel, /const \[panel\] = useState\(\(\) => auxChatController\.createPanel\(\)\);/);
  assert.match(auxChatPanel, /useEffect\(\(\) => panel\.subscribe\(setView\), \[panel\]\);/);
  assert.match(auxChatPanel, /useEffect\(\(\) => \(\) => panel\.dispose\(\), \[panel\]\);/);
});

test('the bind effect refreshes the bridge and binds the task', () => {
  assert.match(
    auxChatPanel,
    /panel\.setBridge\(\s*auxChat,\s*\(callback\) => bridge\.state\.subscribeMany\(\['chat'\], callback\),\s*\);\s*panel\.bind\(sessionId\);\s*\}, \[panel, auxChat, sessionId\]\);/,
    'the bind effect must refresh the bridge and bind the session',
  );
});

test('the per-task registries and the async state machine stay in the controller, not the JSX', () => {
  // The registry Maps and the per-instance async refs must NOT come back into
  // the JSX — they live in the controller, where the executing tests drive
  // them. (The round-30 B1 extraction guard; M6 deleted the stuck family —
  // discardInFlightByTask/discardStuckByTask/sentTextByTask/sentQuotesByTask/
  // restartDiscardFailedByTask/restartWindowKeptAckByTask/
  // discardStuckListenersByTask exist nowhere now.)
  assert.doesNotMatch(
    auxChatPanelRaw,
    /sendInFlightByTask|resetInFlightByTask|draftByTask|restartEpochByTask|draftDeleteListenersByTask/,
    'the per-task registries must stay in the controller, not the JSX',
  );
  assert.doesNotMatch(
    auxChatPanelRaw,
    /generationRef|sendingRef|sessionIdRef|auxIdRef|hasSendContent|consumeSentDraft|withSettleBound/,
    'the async state machine must stay in the controller, not the JSX',
  );
});

// ── Event guards and composer wiring ──

test('Enter guards: key-repeat and IME composition precede the dispatch (round-7 M-A)', () => {
  assert.match(auxChatPanel, /if \(event\.repeat\) return;/);
  assert.match(auxChatPanel, /if \(event\.key !== 'Enter' \|\| event\.shiftKey \|\| isImeComposing\(event\)\) return;/);
  assert.match(auxChatPanel, /void panel\.send\(\);/);
});

test('the composer caps input like the main one (round-20 minor-7)', () => {
  // The aux composer caps input like the main one: drafts persist per task in
  // the controller, so an unbounded paste would live in memory indefinitely.
  assert.match(auxChatPanel, /panel\.setDraftText\(constrainChatInput\(event\.target\.value\)\.text\);/);
});

test('sendInFlight is part of the disabled set and quote-only sends stay possible (round-29 M2)', () => {
  // sendInFlight included in the disabled set: the send latch already makes
  // Enter a silent no-op through the dispatch window; an enabled-looking
  // button doing the same just hid that state.
  assert.match(auxChatPanel, /const composerDisabled = !auxChat \|\| !view\.auxId \|\| busy \|\| view\.restarting \|\| sendInFlight;/);
  // Quote-only send affordance: with staged quotes the composer stays usable
  // even while the draft is empty.
  assert.match(auxChatPanel, /disabled=\{composerDisabled \|\| \(!view\.draft\.trim\(\) && view\.quotes\.length === 0\)\}/);
});

test('New Topic goes through the two-step confirm and stays disabled while restarting', () => {
  assert.match(auxChatPanel, /onClick=\{\(\) => \{ void panel\.restart\(\); \}\}/);
  assert.match(auxChatPanel, /disabled=\{!auxChat \|\| view\.restarting\}/);
  assert.match(auxChatPanel, /view\.restartArmed \? copy\.newTopicConfirm : copy\.newTopic/);
});

// ── View-state rendering ──

test('the binding-pending hint drives the timeline copy (round-12 UX)', () => {
  assert.match(auxChatPanel, /view\.bindingPending \? copy\.bindingHint : copy\.emptyState/);
});

test('busy, in-flight and failure banners render straight from the controller view', () => {
  assert.match(auxChatPanel, /\{busy && \(/);
  assert.match(auxChatPanel, /\{sendInFlight && !busy && \(/);
  assert.match(auxChatPanel, /\{view\.sendFailed && \(/);
  assert.match(auxChatPanel, /\{view\.ensureFailed && \(/);
  assert.match(auxChatPanel, /\{view\.discardFailed && \(/);
});

test('M6: the stuck banner died with the two-invoke restart', () => {
  assert.doesNotMatch(auxChatPanel, /discardStuck/);
  assert.doesNotMatch(auxChatPanelRaw, /aux-chat-discard-stuck/);
  assert.match(auxChatPanel, /copy\.discardFailed/);
});

test('the timeline host and the interactive elements carry their test ids', () => {
  assert.match(auxChatPanel, /copy=\{conversationCopy\}/);
  assert.match(auxChatPanel, /panelId="aux-chat"/);
  assert.match(auxChatPanel, /data-testid="aux-chat-new-topic"/);
  assert.match(auxChatPanel, /data-testid="aux-chat-input"/);
  assert.match(auxChatPanel, /data-testid="aux-chat-send"/);
});

test('quote chips render from the staged quotes and remove through the store', () => {
  assert.match(auxChatPanel, /data-testid="aux-quote-chips"/);
  assert.match(auxChatPanel, /data-testid="aux-quote-remove"/);
  assert.match(auxChatPanel, /copy\.quoteChipCount\(view\.quotes\.length\)/);
  assert.match(auxChatPanel, /removeAuxQuote\(sessionId, index\)/);
});

// ── Scroll management (stays in the JSX by design) ──

test('aux timeline scroll mirrors the main conversation autoScrollRef pattern (round-20 minor-6)', () => {
  // A scroll listener derives the follow flag through the shared transition
  // helper, a rebind re-arms it at the tail, and content growth snaps only
  // while following.
  assert.match(auxChatPanel, /const autoScrollRef = useRef\(true\);/);
  assert.match(auxChatPanel, /transitionConversationScrollState\(\{/);
  assert.match(auxChatPanel, /autoScrollRef\.current = transition\.following;/);
  assert.match(auxChatPanel, /autoScrollRef\.current = true;\s*\}, \[view\.auxId\]\);/);
  assert.match(auxChatPanel, /if \(el && autoScrollRef\.current\) el\.scrollTop = el\.scrollHeight;\s*\}, \[view\.auxId, view\.snapshot\]\);/);
});

test('the scroll-follow listener is keyed on view.auxId so it attaches once the dock portal exists (round-28 B2)', () => {
  assert.match(
    auxChatPanel,
    /el\.addEventListener\('scroll', onScroll, \{ passive: true \}\);\s*return \(\) => el\.removeEventListener\('scroll', onScroll\);\s*\}, \[view\.auxId\]\);/,
    'the scroll-follow listener must be keyed on view.auxId so it attaches once the dock portal exists (round-28 B2)',
  );
});

// ── Controller module shape ──

test('the controller factory takes injected effects and exports the send-content predicate', () => {
  // The factory takes injected effects so the executing tests can drive a fake
  // bridge and fake timers; the panel uses the defaults.
  assert.match(controller, /export function createAuxChatController\(options = \{\}\) \{/);
  assert.match(controller, /export const hasSendContent = \(text, quoteBlock\) => Boolean\(text \|\| quoteBlock\);/);
});

// ── Work-mode (ChatView) aux entry gates ──

test('ChatView aux entry pill reflects real dock visibility (round-9 minor-2)', () => {
  assert.match(chat, /data-testid="aux-chat-open"/);
  // Work-mode entry parity with code mode: the entry pill reflects the
  // panel's real dock visibility via onActiveChange — no highlight while
  // another dock panel occludes the aux panel.
  assert.match(chat, /onActiveChange=\{setAuxChatDockActive\}/);
  assert.match(chat, /auxChatPanel && auxChatDockActive/);
});

test('ChatView panel mount gate and dock-highlight reset carry the bridge conjuncts (round-26 minor M4)', () => {
  assert.match(
    chat,
    /\{auxChatPanel && activeSessionId && !activeSessionId\.startsWith\('sched-'\)\s*&& bridge\.available && bridge\.auxChat && \(/,
    'the ChatView aux panel mount gate must include the bridge conjuncts (round-26 minor M4)',
  );
  assert.match(
    chat,
    /if \(auxChatPanel && activeSessionId && !activeSessionId\.startsWith\('sched-'\)\s*&& bridge\.available && bridge\.auxChat\) return;/,
    'the ChatView aux dock-highlight reset must mirror the mount conjuncts (round-26 minor M4)',
  );
});

// ── Code-mode (CodexAcpView) aux entry gates ──

test('CodexAcpView aux entry is native-agent-only (round-18 must-land)', () => {
  assert.match(codex, /data-testid="aux-chat-open"/);
  assert.match(codex, /t\.uiAuxChat\.openLabel/);
  // The code-mode aux entry must be native-agent-only: an external-ACP task's
  // side chat would silently answer on Pinvou's internal default model while
  // the panel copy implies the task's own assistant.
  assert.match(codex, /\{activeSession && isNativeAgent && bridge\.available && bridge\.auxChat && \(/);
});

test('CodexAcpView quote-selection feed and panel mount carry the same gate (round-19 hardening)', () => {
  // The same gate must hold on the two paths that were reachable without it:
  // the quote-selection feed (a null sessionId suppresses the popover
  // entirely) and the panel mount.
  assert.match(codex, /sessionId=\{activeSession && isNativeAgent && bridge\.available && bridge\.auxChat \? activeSession\.id : null\}/);
  assert.match(codex, /\{auxChatPanel && activeSession && isNativeAgent && bridge\.available && bridge\.auxChat && \(/);
});

test('CodexAcpView dock-highlight reset gates on the same condition (round-25 minor)', () => {
  assert.match(codex, /if \(auxChatPanel && activeSession && isNativeAgent && bridge\.available && bridge\.auxChat\) return;/);
  assert.match(codex, /<AuxChatPanel/);
});

// ── ConversationTimeline aux quote chips ──

test('ConversationTimeline renders aux quote chips in the user bubble', () => {
  // Aux quote chips (selected-text quoting) in the user bubble: the aux
  // projection strips the inline userselect block from userText and hands the
  // excerpts over as userQuotes, so this render branch is the only place the
  // quoted content is still visible. If it regresses, quotes disappear from
  // the transcript silently — the raw block is already stripped and no raw
  // text remains.
  assert.match(conversation, /const userQuotes = Array\.isArray\(turn\.userQuotes\) \? turn\.userQuotes : \[\];/);
  assert.match(conversation, /turn\.userText \|\| userAttachments\.length \|\| userQuotes\.length/);
  // Round-36: the chips render through the extracted ConversationUserQuotes
  // component (the aux branch pushed the turn renderer over the
  // cognitive-complexity cap); the map + testid moved into it.
  assert.match(conversation, /function ConversationUserQuotes\(/);
  assert.match(conversation, /<ConversationUserQuotes quotes=\{userQuotes\} hasBody=\{Boolean\(turn\.userText\)\} \/>/);
  assert.match(conversation, /data-testid="conversation-user-quote"/);
});

// ── Quote selection popover wiring ──

test('a duplicate quote surfaces the trilingual notice instead of silent success (round-30 D5)', () => {
  // The executing identity matrix lives in aux_quote.test.mjs; this pins only
  // the JSX wiring it cannot see.
  assert.match(
    auxQuoteSelection,
    /if \(result\.duplicate\) \{[\s\S]{0,300}?copy\.quoteDuplicate[\s\S]{0,300}?if \(onQuote\) onQuote\(\);\s*return;\s*\}/,
    'a duplicate quote must surface the trilingual notice instead of reporting silent success (round-30 D5)',
  );
});

test('both quote notice branches cancel the armed deferred evaluation (round-33 MAJOR-1)', () => {
  // mouseup precedes click: clicking the chip arms evaluateTimerRef before
  // handleQuote runs, and the DOM selection survives (onMouseDown
  // preventDefaults) — so the over-limit and duplicate branches must cancel
  // the pending evaluation themselves, or the deferred evaluateSelection
  // overwrites the notice with the plain Quote affordance one macrotask
  // later. The success path goes through hidePopover, which already cancels.
  const handleQuote = auxQuoteSelection.slice(auxQuoteSelection.indexOf('const handleQuote'));
  assert.match(handleQuote, /if \(!result\.ok\) \{[\s\S]{0,400}?cancelPendingEvaluation\(\);/);
  assert.match(handleQuote, /if \(result\.duplicate\) \{[\s\S]{0,400}?cancelPendingEvaluation\(\);/);
  assert.match(auxQuoteSelection, /const hidePopover = useCallback\(\(\) => \{[\s\S]{0,300}?cancelPendingEvaluation\(\);/);
});

test('defense-in-depth conjuncts stay pinned as such (round-33 MAJOR-4 residue)', () => {
  // Mutation-sweep verdict at this head: these two guards are not killable
  // through public behavior — the watchdog's binding half never differs from
  // its generation half in any reachable flow (a binding change implies a
  // generation change; the derived aux id makes the converse invisible), and
  // the draft-delete listener's task recheck is unreachable because the
  // registration is per-task and unsubscribed on rebind. Their combined
  // deletion IS executing-test red, and each stays pinned here as documented
  // defense-in-depth (the same treatment commit 9ee1c8609 applied elsewhere).
  // Anchor-resolution guards (the vacuous-slice class right_dock_occlusion_
  // gate pins): a renamed anchor would make indexOf return -1 and the slice
  // run to near-EOF, passing on unrelated copies (round-32 review minor 15).
  const watchdogStart = controller.indexOf('armSendWatchdog(sentTaskId, sendPromise');
  const watchdogEnd = controller.indexOf('await sendPromise;');
  assert.ok(
    watchdogStart >= 0 && watchdogEnd > watchdogStart,
    'watchdog callback anchors must resolve (a vacuous slice would pass on unrelated copies)',
  );
  const watchdog = controller.slice(watchdogStart, watchdogEnd);
  assert.match(watchdog, /if \(!sendingLatch\) return;/);
  assert.match(watchdog, /if \(generation !== sendGeneration \|\| view\.auxId !== sentAuxId\) return;/);
  assert.match(watchdog, /if \(auxChatBusy\(normalizeAuxSnapshot\(auxChat\.snapshot\(sentAuxId\)\)\)\) return;/);
  const listenerStart = controller.indexOf('draftDeleteUnsubscribe = subscribeTaskListeners');
  const listenerEnd = controller.indexOf('if (!auxChat || !sessionId) return;');
  assert.ok(
    listenerStart >= 0 && listenerEnd > listenerStart,
    'draft-listener anchors must resolve (a vacuous slice would pass on unrelated copies)',
  );
  const draftListener = controller.slice(listenerStart, listenerEnd);
  assert.match(draftListener, /if \(sessionIdMirror !== sessionId\) return;/);
  assert.match(draftListener, /view\.draft = '';/);
});
