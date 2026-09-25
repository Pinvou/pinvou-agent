import { projectDeepSeekConversation } from '../conversation/deepseek-conversation.js';
import { parseAuxQuotedMessage } from './aux-quote.mjs';

/**
 * Pure logic layer for the aux chat panel: normalizes the synchronous
 * snapshot from bridge.auxChat.snapshot(auxId), decides busy/empty states,
 * and projects it into the turns ConversationTimeline needs. The projection
 * reuses the main conversation's projectDeepSeekConversation directly — it
 * is a pure function (all inputs come in via parameters, it never reads
 * active-session global state), and the aux session's chatItems are written
 * by the same event pipeline, so the structure matches; only
 * thinking/tokens/timelineEvents are main-session-only enhancements that a
 * background snapshot cannot provide, and aux chat does not project them.
 */

const EMPTY_AUX_SNAPSHOT = Object.freeze({ chatItems: [], busy: false, queued: [] });

export function normalizeAuxSnapshot(raw) {
  if (!raw || typeof raw !== 'object') return EMPTY_AUX_SNAPSHOT;
  return {
    chatItems: Array.isArray(raw.chatItems) ? raw.chatItems : [],
    busy: !!raw.busy,
    queued: Array.isArray(raw.queued) ? raw.queued : [],
  };
}

// The chat domain's notify also carries the main conversation's streaming
// ticks; skip setSnapshot when the aux session's snapshot is unchanged so
// every token does not trigger a panel re-render and turns re-projection.
// Compare item fields one by one with a shallow comparison: the bridge's
// snapshot() shallow-copies each item, and after a streaming delta mutates
// a buffer item in place, two pulls return new objects with different
// content — so field comparison is a true content comparison, and equality
// genuinely means the content did not change.
function auxItemsEqual(left, right) {
  if (left === right) return true;
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false;
  const leftKeys = Object.keys(left);
  if (leftKeys.length !== Object.keys(right).length) return false;
  return leftKeys.every((key) => Object.is(left[key], right[key]));
}

export function auxSnapshotsEqual(prev, next) {
  if (prev === next) return true;
  const a = normalizeAuxSnapshot(prev);
  const b = normalizeAuxSnapshot(next);
  if (a.busy !== b.busy) return false;
  if (a.chatItems.length !== b.chatItems.length || a.queued.length !== b.queued.length) return false;
  for (let i = 0; i < a.chatItems.length; i += 1) {
    if (!auxItemsEqual(a.chatItems[i], b.chatItems[i])) return false;
  }
  for (let i = 0; i < a.queued.length; i += 1) {
    if (!auxItemsEqual(a.queued[i], b.queued[i])) return false;
  }
  return true;
}

// Same rejection criteria as bridge send: busy or queued messages remaining
// both count as not sendable.
export function auxChatBusy(snapshot) {
  const snap = normalizeAuxSnapshot(snapshot);
  return snap.busy || snap.queued.length > 0;
}

// The landing explanation bar is only shown while there is no Q&A content
// yet (system/tool-type items do not count as content).
export function auxChatHasContent(snapshot) {
  return normalizeAuxSnapshot(snapshot).chatItems.some((item) => (
    item && (item.type === 'user' || item.type === 'assistant')
  ));
}

export function projectAuxChatTurns(snapshot, auxId) {
  const snap = normalizeAuxSnapshot(snapshot);
  const turns = projectDeepSeekConversation({
    chatItems: snap.chatItems,
    busy: snap.busy,
    sessionId: auxId,
  }).turns;
  // Sent aux messages carry staged quotes inline as a fenced userselect
  // block (see aux-quote.mjs). Strip the block from the visible user text and
  // attach the excerpts so the timeline bubble renders quote chips instead of
  // raw JSON. Messages without a block pass through untouched.
  return turns.map((turn) => {
    if (!turn || typeof turn.userText !== 'string' || !turn.userText) return turn;
    const { visibleText, quotes } = parseAuxQuotedMessage(turn.userText);
    if (!quotes.length) return turn;
    return { ...turn, userText: visibleText, userQuotes: quotes };
  });
}
