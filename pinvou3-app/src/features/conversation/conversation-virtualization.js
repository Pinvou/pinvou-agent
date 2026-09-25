// Up to 80 turns, normal flow keeps short sessions simple and preserves native
// find-in-page. Above it, resident DOM grows enough in measured mixed-content
// sessions to justify virtualization. This count-based threshold is deliberately
// predictable; unusually tool-heavy shorter sessions remain a follow-up target.
export const CONVERSATION_VIRTUALIZATION_THRESHOLD = 80;

/**
 * @param {number} turnCount - number of projected conversation turns
 * @param {{ current: HTMLElement | null } | null | undefined} scrollElementRef - caller scroll viewport; absent disables virtualization
 * @param {number} threshold - turn count above which history is virtualized
 * @returns {boolean} whether the timeline should render through the virtualizer
 */
export function shouldVirtualizeConversationTurns(turnCount, scrollElementRef, threshold = CONVERSATION_VIRTUALIZATION_THRESHOLD) {
  return Boolean(scrollElementRef && turnCount > threshold);
}

/**
 * @template T extends { status?: string, completedAt?: number | null }
 * @param {T[]} turns - projected conversation turns
 * @param {boolean} virtualized - whether the timeline is virtualized
 * @param {boolean} busy - whether the conversation is currently running a turn
 * @returns {{ historyTurns: T[], liveTurn: T | null, liveTurnIndex: number | null }} virtual history plus the normal-flow live tail
 */
export function splitConversationLiveTail(turns, virtualized, busy = false) {
  const tail = turns[turns.length - 1];
  const tailIsLive = tail?.status === 'running'
    || (busy && !tail?.completedAt);
  if (!virtualized || !turns.length || !tailIsLive) {
    return { historyTurns: turns, liveTurn: null, liveTurnIndex: null };
  }
  return {
    historyTurns: turns.slice(0, -1),
    liveTurn: turns[turns.length - 1],
    liveTurnIndex: turns.length - 1,
  };
}
