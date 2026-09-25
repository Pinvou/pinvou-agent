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

// The controller is a module-scope singleton ON PURPOSE: the registries
// track backend-scoped operations that outlive any panel instance, so all
// mounted panels must share one controller.
assert.match(auxChatPanel, /const auxChatController = createAuxChatController\(\);/);
assert.match(auxChatPanel, /import \{ createAuxChatController, removeAuxQuote \} from '\.\/aux-chat-controller\.mjs';/);
// One controller panel per mounted instance, stable across renders, with the
// view mirrored into React state and every subscription disposed on unmount.
assert.match(auxChatPanel, /const \[panel\] = useState\(\(\) => auxChatController\.createPanel\(\)\);/);
assert.match(auxChatPanel, /useEffect\(\(\) => panel\.subscribe\(setView\), \[panel\]\);/);
assert.match(auxChatPanel, /useEffect\(\(\) => \(\) => panel\.dispose\(\), \[panel\]\);/);
// The bind effect refreshes the bridge and binds the task.
assert.match(
  auxChatPanel,
  /panel\.setBridge\(\s*auxChat,\s*\(callback\) => bridge\.state\.subscribeMany\(\['chat'\], callback\),\s*\);\s*panel\.bind\(sessionId\);\s*\}, \[panel, auxChat, sessionId\]\);/,
  'the bind effect must refresh the bridge and bind the session',
);
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

// ── Event guards and composer wiring ──

// In-flight send Enter guard (round-7 M-A): key-repeat Enter is ignored
// outright; the IME composition guard precedes the dispatch.
assert.match(auxChatPanel, /if \(event\.repeat\) return;/);
assert.match(auxChatPanel, /if \(event\.key !== 'Enter' \|\| event\.shiftKey \|\| isImeComposing\(event\)\) return;/);
assert.match(auxChatPanel, /void panel\.send\(\);/);
// The aux composer caps input like the main one (round-20 minor-7): drafts
// persist per task in the controller, so an unbounded paste would live in
// memory indefinitely.
assert.match(auxChatPanel, /panel\.setDraftText\(constrainChatInput\(event\.target\.value\)\.text\);/);
// sendInFlight included in the disabled set (round-29 M2): the send latch
// already makes Enter a silent no-op through the dispatch window; an
// enabled-looking button doing the same just hid that state.
assert.match(auxChatPanel, /const composerDisabled = !auxChat \|\| !view\.auxId \|\| busy \|\| view\.restarting \|\| sendInFlight;/);
// Quote-only send affordance: with staged quotes the composer stays usable
// even while the draft is empty.
assert.match(auxChatPanel, /disabled=\{composerDisabled \|\| \(!view\.draft\.trim\(\) && view\.quotes\.length === 0\)\}/);
// New Topic goes through the controller's two-step confirm and stays
// disabled while a restart is in flight.
assert.match(auxChatPanel, /onClick=\{\(\) => \{ void panel\.restart\(\); \}\}/);
assert.match(auxChatPanel, /disabled=\{!auxChat \|\| view\.restarting\}/);
assert.match(auxChatPanel, /view\.restartArmed \? copy\.newTopicConfirm : copy\.newTopic/);

// ── View-state rendering ──

// The binding-pending hint (round-12 UX) drives the timeline copy instead of
// the empty state while a binding is being acquired.
assert.match(auxChatPanel, /view\.bindingPending \? copy\.bindingHint : copy\.emptyState/);
// Busy and in-flight-send hints; the failure banners render straight from
// the controller's view.
assert.match(auxChatPanel, /\{busy && \(/);
assert.match(auxChatPanel, /\{sendInFlight && !busy && \(/);
assert.match(auxChatPanel, /\{view\.sendFailed && \(/);
assert.match(auxChatPanel, /\{view\.ensureFailed && \(/);
assert.match(auxChatPanel, /\{view\.discardFailed && \(/);
// M6: the stuck banner died with the two-invoke restart — the atomic reset
// has no wedged-discard state to surface.
assert.doesNotMatch(auxChatPanel, /discardStuck/);
assert.doesNotMatch(auxChatPanelRaw, /aux-chat-discard-stuck/);
assert.match(auxChatPanel, /copy\.discardFailed/);
assert.match(auxChatPanel, /copy=\{conversationCopy\}/);
assert.match(auxChatPanel, /panelId="aux-chat"/);
assert.match(auxChatPanel, /data-testid="aux-chat-new-topic"/);
assert.match(auxChatPanel, /data-testid="aux-chat-input"/);
assert.match(auxChatPanel, /data-testid="aux-chat-send"/);
// Conversation quotes: chips render from the staged quotes and remove
// through the store (the subscription on the controller side repaints).
assert.match(auxChatPanel, /data-testid="aux-quote-chips"/);
assert.match(auxChatPanel, /data-testid="aux-quote-remove"/);
assert.match(auxChatPanel, /copy\.quoteChipCount\(view\.quotes\.length\)/);
assert.match(auxChatPanel, /removeAuxQuote\(sessionId, index\)/);

// ── Scroll management (stays in the JSX by design) ──

// Aux timeline scroll (round-20 minor-6): mirror the main conversation's
// autoScrollRef pattern — a scroll listener derives the follow flag through
// the shared transition helper, a rebind re-arms it at the tail, and content
// growth snaps only while following.
assert.match(auxChatPanel, /const autoScrollRef = useRef\(true\);/);
assert.match(auxChatPanel, /transitionConversationScrollState\(\{/);
assert.match(auxChatPanel, /autoScrollRef\.current = transition\.following;/);
assert.match(auxChatPanel, /autoScrollRef\.current = true;\s*\}, \[view\.auxId\]\);/);
assert.match(auxChatPanel, /if \(el && autoScrollRef\.current\) el\.scrollTop = el\.scrollHeight;\s*\}, \[view\.auxId, view\.snapshot\]\);/);
// Round-28 B2: the scroll-follow listener must re-attach when the dock
// portal exists — keyed on view.auxId, not [].
assert.match(
  auxChatPanel,
  /el\.addEventListener\('scroll', onScroll, \{ passive: true \}\);\s*return \(\) => el\.removeEventListener\('scroll', onScroll\);\s*\}, \[view\.auxId\]\);/,
  'the scroll-follow listener must be keyed on view.auxId so it attaches once the dock portal exists (round-28 B2)',
);

// ── Controller module shape ──

// The factory takes injected effects so the executing tests can drive a fake
// bridge and fake timers; the panel uses the defaults.
assert.match(controller, /export function createAuxChatController\(options = \{\}\) \{/);
assert.match(controller, /export const hasSendContent = \(text, quoteBlock\) => Boolean\(text \|\| quoteBlock\);/);

// ── Work-mode (ChatView) aux entry gates ──

assert.match(chat, /data-testid="aux-chat-open"/);
// Work-mode entry parity with code mode (round-9 minor-2): the entry pill
// reflects the panel's real dock visibility via onActiveChange — no
// highlight while another dock panel occludes the aux panel.
assert.match(chat, /onActiveChange=\{setAuxChatDockActive\}/);
assert.match(chat, /auxChatPanel && auxChatDockActive/);
// Round-26 minor M4: ChatView's panel mount gate and dock-highlight reset
// must carry the same four conjuncts as the entry button (sched- exclusion,
// active session, bridge.available, bridge.auxChat).
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

// ── Code-mode (CodexAcpView) aux entry gates ──

assert.match(codex, /data-testid="aux-chat-open"/);
assert.match(codex, /t\.uiAuxChat\.openLabel/);
// The code-mode aux entry must be native-agent-only (round-18 must-land): an
// external-ACP task's side chat would silently answer on Pinvou's internal
// default model while the panel copy implies the task's own assistant.
assert.match(codex, /\{activeSession && isNativeAgent && bridge\.available && bridge\.auxChat && \(/);
// The same gate must hold on the two paths that were reachable without it
// (round-19 hardening): the quote-selection feed (a null sessionId
// suppresses the popover entirely) and the panel mount.
assert.match(codex, /sessionId=\{activeSession && isNativeAgent && bridge\.available && bridge\.auxChat \? activeSession\.id : null\}/);
assert.match(codex, /\{auxChatPanel && activeSession && isNativeAgent && bridge\.available && bridge\.auxChat && \(/);
// The dock-highlight reset effect must gate on the SAME condition (round-25
// minor consistency note).
assert.match(codex, /if \(auxChatPanel && activeSession && isNativeAgent && bridge\.available && bridge\.auxChat\) return;/);
assert.match(codex, /<AuxChatPanel/);

// ── ConversationTimeline aux quote chips ──

// Aux quote chips (selected-text quoting) in the user bubble: the aux
// projection strips the inline userselect block from userText and hands the
// excerpts over as userQuotes, so this render branch is the only place the
// quoted content is still visible. If it regresses, quotes disappear from
// the transcript silently — the raw block is already stripped and no raw
// text remains.
assert.match(conversation, /const userQuotes = Array\.isArray\(turn\.userQuotes\) \? turn\.userQuotes : \[\];/);
assert.match(conversation, /turn\.userText \|\| userAttachments\.length \|\| userQuotes\.length/);
assert.match(conversation, /userQuotes\.map\(\(quote, index\)/);
assert.match(conversation, /data-testid="conversation-user-quote"/);

// ── Quote selection popover wiring ──

// A duplicate quote must surface the trilingual notice instead of reporting
// silent success (round-30 D5); the executing identity matrix lives in
// aux_quote.test.mjs, this pins only the JSX wiring it cannot see.
assert.match(
  auxQuoteSelection,
  /if \(result\.duplicate\) \{[\s\S]{0,300}?copy\.quoteDuplicate[\s\S]{0,300}?if \(onQuote\) onQuote\(\);\s*return;\s*\}/,
  'a duplicate quote must surface the trilingual notice instead of reporting silent success (round-30 D5)',
);

console.log('Aux chat panel contract tests passed');
